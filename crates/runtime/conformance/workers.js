// Worker — the constructor's contract, and the surface a `Worker` object has.
//
// Everything here is observable *without* driving the event loop, which is the
// same split `channel.js` makes: starting an agent, delivering a message and
// reporting a failure all need a real loop, and are covered end-to-end by
// `crates/runtime-cli/tests/workers.rs` against the real CLI.
//
// What that leaves is worth pinning down on its own, because it is where the
// non-standard options live: a malformed one has to be rejected *by the call
// that made it*, not reported asynchronously through `onerror`, and every
// rejection here is a rule someone could otherwise trip silently.

test("Worker is a constructor on every agent", () => {
  assert(typeof Worker === "function");
  assertEquals(Worker.prototype[Symbol.toStringTag], "Worker");
  assert(Worker.prototype instanceof EventTarget);
});

test("Worker requires a script URL", () => {
  assertThrows(() => new Worker(), "TypeError");
});

// Every input to this runtime is a module (SPEC §8), so there is no
// classic-script path for a classic worker to use. Deno refuses them too.
test("only module workers exist", () => {
  assertThrows(() => new Worker("./w.js", { type: "classic" }), "TypeError");
});

// ---- the non-standard options ----------------------------------------------
//
// A malformed option is a bad argument, and a bad argument throws from the call
// that made it. Only a worker that *fails to start* reports asynchronously.

test("permissions must be \"inherit\" or an array of names", () => {
  assertThrows(() => new Worker("./w.js", { permissions: "net" }), "TypeError");
  assertThrows(() => new Worker("./w.js", { permissions: 7 }), "TypeError");
});

// An unknown name throws rather than being skipped: dropping it fails closed,
// which sounds harmless until the worker takes the degraded path forever and
// the denial surfaces three layers from the typo. `permissions.has()` refuses
// to answer `false` for an unknown name for exactly this reason.
test("an unknown permission name is refused, not dropped", () => {
  assertThrows(() => new Worker("./w.js", { permissions: ["nett"] }), "TypeError");
  assertThrows(() => new Worker("./w.js", { permissions: ["read", "wrote"] }), "TypeError");
});

test("memory is a positive whole number of megabytes", () => {
  assertThrows(() => new Worker("./w.js", { memory: "64MB" }), "TypeError");
  assertThrows(() => new Worker("./w.js", { memory: 0 }), "TypeError");
  assertThrows(() => new Worker("./w.js", { memory: -1 }), "TypeError");
  assertThrows(() => new Worker("./w.js", { memory: 1.5 }), "TypeError");
});

test("env is \"inherit\" or an object of variables", () => {
  assertThrows(() => new Worker("./w.js", { env: "share" }), "TypeError");
  assertThrows(() => new Worker("./w.js", { env: 1 }), "TypeError");
});

// ---- the object a spawn returns --------------------------------------------
//
// Asserted against a worker whose module does not exist: the constructor is not
// allowed to throw for a script that fails to fetch, so the object is fully
// formed either way, and the failure arrives later through `onerror`.

test("a Worker has the messaging and lifetime surface", () => {
  const w = new Worker("./does-not-exist.mjs");
  w.onerror = (event) => event.preventDefault();
  for (const method of ["postMessage", "terminate", "ref", "unref"]) {
    assertEquals(typeof w[method], "function");
  }
  for (const handler of ["onmessage", "onmessageerror", "onerror"]) {
    assert(handler in w);
  }
  assertEquals(typeof w.queued, "number");
  w.terminate();
});

// The backpressure signal. Nothing refuses a message — HTML does not permit
// `postMessage` to fail for queue depth — so this is what a producer that
// outruns its worker paces itself against.
//
// Only the resting value is asserted here. The depth rising with a backlog and
// falling as the worker drains needs an agent on the other end, and is covered
// by `a_worker_reports_how_deep_its_inbox_is` in the CLI tests.
test("queued is zero for a worker with nothing outstanding", () => {
  const w = new Worker("./does-not-exist.mjs");
  w.onerror = (event) => event.preventDefault();
  assertEquals(w.queued, 0);
  w.terminate();
  // Terminating discards whatever was never delivered.
  assertEquals(w.queued, 0);
});

test("terminate is idempotent, and postMessage after it is a no-op", () => {
  const w = new Worker("./does-not-exist.mjs");
  w.onerror = (event) => event.preventDefault();
  w.terminate();
  w.terminate();
  w.postMessage("dropped");
  assertEquals(w.queued, 0);
});

test("ref and unref are idempotent", () => {
  const w = new Worker("./does-not-exist.mjs");
  w.onerror = (event) => event.preventDefault();
  w.unref();
  w.unref();
  w.ref();
  w.ref();
  w.terminate();
});

// A non-cloneable value is rejected at the call, as `MessagePort` rejects one:
// the clone happens synchronously, so the failure belongs to the caller.
test("postMessage rejects a non-cloneable value at the call site", () => {
  const w = new Worker("./does-not-exist.mjs");
  w.onerror = (event) => event.preventDefault();
  assertThrows(() => w.postMessage(() => {}), "DataCloneError");
  w.terminate();
});

// ---- which agent this is ---------------------------------------------------
//
// There is no `isMainThread`: a worker is recognised by the shape of its global
// scope, in HTML as in Deno and Bun. This file runs on the agent driving the
// process, so the worker interfaces must be absent from it — the same test run
// inside a worker is `wpt/`'s job, where every file runs in both modes.

test("the driver agent is not a worker", () => {
  // The absence *is* the test: these four exist only inside a worker, and their
  // presence is how a script tells where it is running.
  assertEquals(typeof globalThis.WorkerGlobalScope, "undefined");
  assertEquals(typeof globalThis.DedicatedWorkerGlobalScope, "undefined");
  assertEquals(typeof globalThis.WorkerNavigator, "undefined");
  assertEquals(typeof globalThis.WorkerLocation, "undefined");
  // `location` names one script, and on the driver agent no one script is *the*
  // script — so there is none.
  assertEquals(typeof globalThis.location, "undefined");
  assertEquals(self, globalThis);
});

// `SharedWorker` shares a worker between documents, and there are none.
test("SharedWorker is not exposed", () => {
  assertEquals(typeof globalThis.SharedWorker, "undefined");
});
