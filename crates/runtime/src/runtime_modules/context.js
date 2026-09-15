// runtime:context — values that follow the work, not the call stack
// (DECISIONS D88, SPEC §11).
//
// A request id, a tenant, an ORM's ambient transaction: things every layer
// underneath needs and no layer wants in its signature. `ctx.run(value, fn)`
// makes a value current for `fn` and everything `fn` schedules; `ctx.get()`
// reads it back, however deep and however many `await`s later.
//
// This module is **ungated**. It carries no ambient authority, performs no I/O,
// and exposes nothing that was not already in the caller's own scope — a
// context is a channel from one piece of a program to another piece of the same
// program. Denying it would not restrict a reach outside the isolate, it would
// only break correctness: an ORM that cannot see its own transaction picks the
// pool instead, and a query answers with the wrong tenant's rows. That is a
// reason to make it unfailable, not a reason to gate it.
//
// # What propagates
//
//   await / .then / queueMicrotask   yes    the mapping at the await
//   setTimeout / setInterval         yes    the mapping at schedule time
//   runtime:* op callbacks           yes    the mapping when the op was issued
//   EventTarget dispatch             NO     the dispatcher's mapping
//   spawned Worker                   NO     a separate agent, all defaults
//
// `EventTarget` is the one a Node user will guess wrong, so to say it plainly:
// a listener runs in the mapping of whoever called `dispatchEvent`, not the one
// that was current when `addEventListener` ran. Listeners are long-lived and
// usually registered at module scope, so capturing at registration would pin a
// request's mapping to a listener that outlives the request — a leak that grows
// for as long as the process runs. Wrap the listener instead:
//
//   target.addEventListener("message", bind(onMessage));
//
// The host half is in `crates/engine/src/async_context.rs`; the two split at a
// value neither of them parses. Everything about what a mapping *is* lives
// here.

const ops = globalThis.__ops;

// Host mapping accessors, installed by the engine (see `async_context.rs`).
// Captured at load so that reassigning the globals cannot reach this module.
const frameOf = globalThis.__ctx_frame;
const swap = globalThis.__ctx_swap;
const taskId = globalThis.__ctx_task;
const parentId = globalThis.__ctx_parent;

// A *mapping* is one of these, or `undefined` for the root — every context at
// its default, no trace of its own. It is opaque to the host, which only ever
// moves it from one scope to another.
//
//   values  Map<Context, value>, or null when the scope added no values
//   trace   the W3C trace id this scope runs under, or null to inherit
//   kind    what established this scope: "main", "http-request", "worker", …
//
// Frozen because it is shared by every scope derived from it: the copy-on-write
// below is the only way to "change" one, and a mapping that could be mutated in
// place would let a child scope rewrite its parent's.
function makeFrame(values, trace, kind) {
  return Object.freeze({ values, trace, kind });
}

// Copy-on-write. `run()` shallow-copies the value map for the new scope, so a
// write in one branch is invisible to a sibling and to the parent — two
// requests in flight cannot see each other's values, whatever order the loop
// interleaves them in.
//
// Shallow-copying a `Map` is O(n) in the number of *contexts*, not in the depth
// of the async chain, and a program has a handful of contexts. A persistent
// structure would turn that O(n) into O(log n) and cost an allocation per node;
// that trade only pays at a context count no real program has, so it is not
// made here.
function derive(previous, kind) {
  const values = previous?.values ? new Map(previous.values) : new Map();
  return makeFrame(values, previous?.trace ?? null, kind ?? previous?.kind ?? ROOT_KIND);
}

// Runs `fn` with `frame` current, then puts back whatever was current before —
// on the throwing path too. Every scope in this module goes through here, which
// is why there is no `enterWith`: a scope you cannot leave has no `finally`.
function within(frame, fn, args, self) {
  const saved = swap(frame);
  try {
    return args === undefined ? fn.call(self) : fn.apply(self, args);
  } finally {
    swap(saved);
  }
}

// ---------------------------------------------------------------------------
// Trace ids
// ---------------------------------------------------------------------------

// W3C trace-context: 32 lowercase hex characters, not all zero.
const TRACE_ID = /^[0-9a-f]{32}$/;
const NULL_TRACE = "00000000000000000000000000000000";

function mintTraceId() {
  const bytes = ops.random_bytes(16);
  let hex = "";
  for (let i = 0; i < bytes.length; i++) hex += bytes[i].toString(16).padStart(2, "0");
  return hex;
}

// The trace for work that is not under an http request or an explicit
// `withTrace` — minted on first read rather than at load, so a program that
// never asks never draws the entropy, and shared from then on so that every
// task in this agent's root reports the same trace.
let ambientTrace = null;

function traceOf(frame) {
  if (frame?.trace) return frame.trace;
  if (ambientTrace === null) ambientTrace = mintTraceId();
  return ambientTrace;
}

// Accepts what a `traceparent` carries, and refuses what it does not. Uppercase
// hex is normalized rather than rejected — some producers emit it, and the W3C
// format is a lowercase *serialization* of a 16-byte id, not a different id.
// The all-zero id is invalid per the spec and is the usual shape of "this field
// was never filled in", so it is refused rather than propagated.
function checkTraceId(value, label) {
  if (typeof value !== "string") {
    throw new TypeError(`${label}: traceId must be a string, got ${typeof value}`);
  }
  const id = value.toLowerCase();
  if (!TRACE_ID.test(id) || id === NULL_TRACE) {
    throw new TypeError(
      `${label}: traceId must be 32 hex characters (W3C trace-id), got ${JSON.stringify(value)}`,
    );
  }
  return id;
}

// ---------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------

// What established the root scope of this agent. A worker's root is its own —
// a spawned worker starts with every context at its default and no inherited
// trace, because it is a separate agent with a separate heap, and auto-injecting
// the parent's mapping across `postMessage` would make a structured-clone
// boundary silently carry values that were never serialized. A worker that
// wants to continue a trace is sent the id and calls `withTrace`.
// `worker_scope_info()` answers `null` on the agent driving the process and an
// object describing the scope inside a worker; the op is absent entirely when an
// embedder installed no worker host.
const ROOT_KIND =
  typeof ops.worker_scope_info === "function" && ops.worker_scope_info() !== null
    ? "worker"
    : "main";

function currentTask() {
  const frame = frameOf();
  const parent = parentId();
  return {
    id: taskId(),
    // The root task is the one nothing scheduled — the only `null` here.
    parentId: parent < 0 ? null : parent,
    traceId: traceOf(frame),
    kind: frame?.kind ?? ROOT_KIND,
  };
}

// ---------------------------------------------------------------------------
// Contexts
// ---------------------------------------------------------------------------

function createContext(options) {
  const o = options ?? {};
  if (o.name !== undefined && typeof o.name !== "string") {
    throw new TypeError(`createContext: name must be a string, got ${typeof o.name}`);
  }
  const name = o.name;
  const defaultValue = o.defaultValue;

  // The context object *is* the key. No string-keyed bag, no registry, no way
  // to enumerate what another library stored — two libraries that both call
  // theirs "user" cannot collide, because neither can name the other's object.
  const context = {
    get name() {
      return name;
    },

    get() {
      const values = frameOf()?.values;
      if (values === undefined || values === null) return defaultValue;
      return values.has(context) ? values.get(context) : defaultValue;
    },

    run(value, fn, ...args) {
      if (typeof fn !== "function") {
        throw new TypeError(`run: fn must be a function, got ${typeof fn}`);
      }
      const frame = derive(frameOf());
      frame.values.set(context, value);
      // Returns whatever `fn` returns, promise included. For an async `fn` this
      // returns as soon as it first yields — the mapping is reinstalled on entry
      // to each continuation, not held until the promise settles, which is what
      // lets two requests be in flight without one's scope outliving the other.
      return within(frame, fn, args);
    },
  };
  return Object.freeze(context);
}

function snapshot() {
  const captured = frameOf();
  return function runInContext(fn, ...args) {
    if (typeof fn !== "function") {
      throw new TypeError(`snapshot: fn must be a function, got ${typeof fn}`);
    }
    return within(captured, fn, args, this);
  };
}

function bind(fn) {
  if (typeof fn !== "function") {
    throw new TypeError(`bind: fn must be a function, got ${typeof fn}`);
  }
  const captured = frameOf();
  // `this` is forwarded: an `EventTarget` listener is called with the target as
  // its receiver, and a bound listener that lost it would not be a drop-in.
  const bound = function (...args) {
    return within(captured, fn, args, this);
  };
  // Keep the shape of what was passed in, so a bound listener still reads as
  // itself in a stack trace and `fn.length` still says what it takes.
  Object.defineProperty(bound, "name", { value: fn.name, configurable: true });
  Object.defineProperty(bound, "length", { value: fn.length, configurable: true });
  return bound;
}

function withTrace(traceId, fn) {
  if (typeof fn !== "function") {
    throw new TypeError(`withTrace: fn must be a function, got ${typeof fn}`);
  }
  const id = checkTraceId(traceId, "withTrace");
  const frame = derive(frameOf());
  return within(makeFrame(frame.values, id, frame.kind), fn);
}

// `runtime:http` reaches this without importing the module: an import would
// turn the promise hook on for every server, including the many that never read
// a context. It calls through `__internal` instead, which is only populated
// once this module has actually been loaded — so a server whose program uses
// contexts gets a trace per request, and one whose program does not pays
// nothing (see `__ctx_enabled`).
//
// A *fresh* mapping, not a derived one: a request is a root. Deriving would
// carry whatever the accept loop happened to be holding into every request
// handler, which is the leak this module exists to avoid.
globalThis.__internal.context.scope = function requestScope(traceId, kind, fn) {
  return within(makeFrame(null, traceId ?? mintTraceId(), kind), fn);
};
globalThis.__internal.context.checkTraceId = checkTraceId;

export { createContext, snapshot, bind, withTrace, currentTask };
export default { createContext, snapshot, bind, withTrace, currentTask };
