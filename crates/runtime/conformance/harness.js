// The in-JS harness every `conformance/*.js` file is written against:
// `test(name, fn)` (sync or async), `todo(name, fn)` for known deviations,
// the `assert*` helpers, and a `__results` tally the runner reads back.
//
// `todo` is the inverse of `test`: the assertion states what the spec requires
// *today*, while the runtime is known not to satisfy it yet. A throwing `todo`
// is tallied as `todo` (not `fail`, so the gate stays green); a *passing* one
// is an error — the deviation is fixed, and the case must be promoted to `test`
// so the behaviour can never silently regress.
//
// This file is loaded by both runners — `conformance_suite_passes` in
// `crates/runtime/src/lib.rs` (the CI gate) and `run.js` (the same suite under
// `esrun`, on a real driver) — so the two can never drift apart. It is not
// itself a suite file; both runners skip it when collecting.
globalThis.__results = { pass: 0, fail: 0, todo: 0, failures: [], fixed: [] };
globalThis.__pending = [];
globalThis.test = (name, fn) => {
  let r;
  try { r = fn(); }
  catch (e) { __results.fail++; __results.failures.push(name + ": " + ((e && e.message) || e)); return; }
  if (r && typeof r.then === "function") {
    __pending.push(r.then(
      () => { __results.pass++; },
      (e) => { __results.fail++; __results.failures.push(name + ": " + ((e && e.message) || e)); },
    ));
  } else { __results.pass++; }
};
globalThis.todo = (name, fn) => {
  let r;
  try { r = fn(); }
  catch { __results.todo++; return; }
  if (r && typeof r.then === "function") {
    __pending.push(r.then(
      () => { __results.fixed.push(name); },
      () => { __results.todo++; },
    ));
  } else { __results.fixed.push(name); }
};
globalThis.assert = (cond, msg) => { if (!cond) throw new Error(msg || "assertion failed"); };
globalThis.assertEquals = (actual, expected, msg) => {
  if (actual !== expected) {
    throw new Error((msg ? msg + ": " : "") + `expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
};
globalThis.assertThrows = (fn, name) => {
  let threw = null;
  try { fn(); } catch (e) { threw = e; }
  if (!threw) throw new Error("expected a throw, but none occurred");
  if (name && threw.name !== name) throw new Error(`expected ${name}, got ${threw.name}`);
};
globalThis.__await_all = () => Promise.all(__pending);
