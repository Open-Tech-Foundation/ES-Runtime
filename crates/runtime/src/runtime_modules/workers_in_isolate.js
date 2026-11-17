// `runtime:workers` inside an `Isolate` (DECISIONS D227).
//
// An isolate computes and reaches out only through the functions its parent
// handed it, so this is the whole module there: `host`, the stub that calls
// them. The durable workers and `Isolate` itself are absent — both need
// capabilities an isolate does not hold, and importing them would compile
// `runtime:db` and `runtime:fs` into every isolate that only wanted `host`.
//
// `__isolate_host` is installed by the worker prelude before any module runs.

const host = globalThis.__isolate_host;

export { host };
export default { host };
