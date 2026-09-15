// runtime:diagnostics — what the runtime is doing, and how long it took
// (DECISIONS D89, SPEC §11).
//
// Spans with deterministic ends, filtered in the host and delivered in batches.
// There is no per-operation JS callback: an event nothing is subscribed to — or
// one every subscription's filter rejects — costs an integer compare in Rust and
// never reaches this module at all. That is the whole design, and it is the
// thing Node's `async_hooks` cannot retrofit.
//
// # Capabilities
//
//   diagnostics          subscribe, inventory, metrics, span — timings, kinds,
//                        counts, scheduler state. `attributes` come back empty.
//   diagnostics-detail   populates `attributes`. Implies `diagnostics`.
//
// `detail` adds no exports of its own; it widens what the first one returns. A
// profiler runs on `diagnostics` alone and sees full timings with empty payloads,
// which is the split that lets you hand profiling to something you would not
// hand your SQL to.
//
// # The three timestamps
//
//   scheduledAt   when the work became runnable
//   startedAt     when it actually started
//   endedAt       when it finished
//
// So `startedAt - scheduledAt` is always queue delay. For a timer, "became
// runnable" is its deadline, not when it was armed — the delay you asked for is
// not a queue. For an op the two are equal, because there is no boundary between
// them the host can see; that is reported as zero rather than invented.
//
// # Loop-tick attribution
//
// Every record carries the `tick` it landed in, and the "tick" kind gives each
// turn of the loop its own record. Read together they separate "this was slow"
// from "this waited behind something else in the same turn" — which, on a
// single-threaded driven loop, is the question timings alone cannot answer.

import { currentTask } from "runtime:context";

const ops = globalThis.__ops;
// The enclosing-span accessor, installed by the engine. A span made current here
// is carried across every `await` by `runtime:context`'s propagation, which is
// what lets an op six continuations deep still nest under the request that
// caused it.
const swapSpan = globalThis.__ctx_swap_span;
const currentSpan = globalThis.__ctx_span;

// Forcing the root trace to exist, once, at load: every record the host writes
// is attributed to a trace, and a program that never reads `currentTask()`
// would otherwise have none to attribute them to.
currentTask();

// id -> onBatch. The host calls `__dispatch_diagnostics` once per subscription
// per tick, never once per record.
const subscribers = new Map();

// Frozen: a batch goes to one subscriber, but the records came from the host and
// a consumer that mutated one would be editing a reading.
function deliver(onBatch, records, dropped) {
  for (const record of records) {
    Object.freeze(record.attributes);
    Object.freeze(record);
  }
  Object.freeze(records);
  try {
    onBatch({ records, dropped });
  } catch (e) {
    // A throwing subscriber must not take down the loop turn that delivered to
    // it. Reported like any other uncaught failure, and the subscription stays:
    // dropping it would lose records for a bug in one batch.
    reportError(e);
  }
}

globalThis.__dispatch_diagnostics = (id, records, dropped) => {
  const onBatch = subscribers.get(id);
  if (onBatch !== undefined) deliver(onBatch, records, dropped);
};

const KINDS = ["op", "timer", "user", "tick"];

function parseFilter(filter) {
  const f = filter ?? {};
  const out = {};
  if (f.kinds !== undefined) {
    if (!Array.isArray(f.kinds)) {
      throw new TypeError(`subscribe: kinds must be an array, got ${typeof f.kinds}`);
    }
    for (const kind of f.kinds) {
      if (!KINDS.includes(kind)) {
        throw new TypeError(
          `subscribe: unknown kind ${JSON.stringify(kind)} (expected one of ${KINDS.join(", ")})`,
        );
      }
    }
    out.kinds = [...f.kinds];
  }
  if (f.minDuration !== undefined) {
    if (typeof f.minDuration !== "number" || !(f.minDuration >= 0)) {
      throw new TypeError("subscribe: minDuration must be a number >= 0");
    }
    out.minDuration = f.minDuration;
  }
  if (f.sample !== undefined) {
    if (typeof f.sample !== "number" || !(f.sample >= 0 && f.sample <= 1)) {
      throw new TypeError("subscribe: sample must be a number between 0 and 1");
    }
    out.sample = f.sample;
  }
  if (f.bufferSize !== undefined) {
    if (!Number.isInteger(f.bufferSize) || f.bufferSize < 1) {
      throw new TypeError("subscribe: bufferSize must be an integer >= 1");
    }
    out.bufferSize = f.bufferSize;
  }
  return out;
}

// Detail is decided by the **host**, not claimed here.
//
// Two ops do the same thing behind different gates: the `detail` one is refused
// unless the run holds that capability, so the fallback below is the capability
// check rather than a report of it. JS cannot ask for payloads it was not
// granted, which it could if this were a boolean argument.
function openSubscription(filter) {
  try {
    return ops.diagnostics_subscribe_detail(filter);
  } catch (e) {
    if (e.name !== "NotAllowedError") throw e;
    return ops.diagnostics_subscribe(filter);
  }
}

function subscribe(filter, onBatch) {
  if (typeof onBatch !== "function") {
    throw new TypeError(`subscribe: onBatch must be a function, got ${typeof onBatch}`);
  }
  const id = openSubscription(parseFilter(filter));
  subscribers.set(id, onBatch);
  let closed = false;
  return Object.freeze({
    close() {
      // Idempotent: closing twice must not unsubscribe an id the host has since
      // handed to somebody else.
      if (closed) return;
      closed = true;
      subscribers.delete(id);
      // Delivery is per turn, so closing inside one would drop everything
      // recorded in it. The host hands the remainder back and it is delivered
      // synchronously here: `close()` means "give me what you have, then stop".
      const final = ops.diagnostics_unsubscribe(id);
      if (final !== null && (final.records.length > 0 || final.dropped > 0)) {
        deliver(onBatch, final.records, final.dropped);
      }
    },
  });
}

function inventory() {
  const { handles } = ops.diagnostics_inventory();
  return Object.freeze({ handles: Object.freeze(handles.map(Object.freeze)) });
}

function metrics() {
  return ops.diagnostics_metrics();
}

// Opens a span. It records its **parent** — the span enclosing it right now —
// but does **not** become the enclosing span itself.
//
// That split is OpenTelemetry's (`startSpan` vs `startActiveSpan`) and it is the
// same reason `runtime:context` has no `enterWith`. Calling an async function
// runs its body synchronously up to the first `await`, so a span that made
// itself active there would still be active when control returned to its
// *caller* — and every later op in the caller would nest under a span that had
// nothing to do with it. Only `run()` below makes one active, for exactly the
// call it wraps.
function openSpan(name, attributes, kind) {
  const [id, startedAt] = ops.diagnostics_span_open();
  if (id === 0) return null;
  const parent = currentSpan();
  const traceId = currentTask().traceId;
  let ended = false;
  const finish = (status) => {
    // Ending twice would record the same work twice; the second call is the bug,
    // and ignoring it is kinder than a throw out of a `finally`.
    if (ended) return;
    ended = true;
    ops.diagnostics_span_close(
      id, parent, name, startedAt, status, attributes ?? {}, traceId, kind ?? "user",
    );
  };
  return {
    id,
    end: () => finish("ok"),
    fail: () => finish("error"),
    cancel: () => finish("cancelled"),
    // Runs `fn` with this span active, so everything `fn` does — and everything
    // it schedules, through `runtime:context`'s propagation — nests under it.
    // The active span is restored **synchronously**, before `fn`'s promise
    // settles: what carries it into the continuations is the capture each
    // promise took, not a value left lying around for the caller to trip over.
    run(fn) {
      const previous = swapSpan(id);
      try {
        const result = fn();
        if (result !== null && typeof result?.then === "function") {
          return result.then(
            (value) => {
              finish("ok");
              return value;
            },
            (error) => {
              finish("error");
              throw error;
            },
          );
        }
        finish("ok");
        return result;
      } catch (e) {
        finish("error");
        throw e;
      } finally {
        swapSpan(previous);
      }
    },
  };
}

// A span the program opens, sharing the id space and the timeline with the
// runtime's own. `source: "user"` is what tells them apart.
//
//   span("checkout").end()                  // times a region; nests nothing
//   span("checkout", {}, () => work())      // …and everything `work` does
//
// With a callback the span is **active** for that call, so the ops inside it
// become its children. Without one it is a plain measurement: it records its own
// parent, and nothing nests under it.
function span(name, options, fn) {
  if (typeof name !== "string") {
    throw new TypeError(`span: name must be a string, got ${typeof name}`);
  }
  const attributes = options?.attributes;
  if (attributes !== undefined && (typeof attributes !== "object" || attributes === null)) {
    throw new TypeError(`span: attributes must be an object, got ${typeof attributes}`);
  }
  if (fn !== undefined && typeof fn !== "function") {
    throw new TypeError(`span: fn must be a function, got ${typeof fn}`);
  }
  const open = openSpan(name, attributes, "user");
  if (fn !== undefined) return open === null ? fn() : open.run(fn);
  const noop = () => {};
  return Object.freeze(
    open === null
      ? { end: noop, fail: noop, cancel: noop }
      : { end: open.end, fail: open.fail, cancel: open.cancel },
  );
}

// `runtime:http` opens a span per request through this rather than importing the
// module, for the reason `runtime:context` gives: an import would enable
// recording for every server in the runtime. Filled in only once this module has
// actually been loaded, and it answers `null` when nothing is watching.
globalThis.__internal.diagnostics.openSpan = (name, attributes) => {
  try {
    return openSpan(name, attributes, "request");
  } catch {
    // The capability is not held. A server must not fail to serve because it
    // could not be observed.
    return null;
  }
};

export { subscribe, inventory, metrics, span };
export default { subscribe, inventory, metrics, span };
