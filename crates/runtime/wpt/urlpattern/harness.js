// A minimal stand-in for WPT's testharness.js — just the assertions
// `urlpatterntests.js` uses, plus a tally the runner prints. Not a general
// harness: it exists so the vendored WPT file below can run unmodified.
// Standalone port of WPT urlpattern/resources/urlpatterntests.js.
// Harness shim: enough of testharness.js for this suite.
// Indirect eval puts these in the global *lexical* scope, which a module
// cannot reach, so the tally is published on globalThis explicitly.
const results = (globalThis.results = []);
let current = null;
function test(fn, name) {
  current = { name, error: null };
  try { fn(); } catch (e) { current.error = (e && e.message) || String(e); }
  results.push(current);
}
class AssertionError extends Error {}
function fail(msg) { throw new AssertionError(msg); }
function assert_equals(actual, expected, msg) {
  if (!Object.is(actual, expected)) {
    fail(`${msg}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}
function assert_throws_js(Ctor, fn, msg) {
  let threw = null;
  try { fn(); } catch (e) { threw = e; }
  if (!threw) fail(`${msg}: expected ${Ctor.name}, but nothing was thrown`);
  if (!(threw instanceof Ctor)) fail(`${msg}: expected ${Ctor.name}, got ${threw.name}`);
}
function assert_object_equals(actual, expected, msg) {
  if (actual === undefined || actual === null) fail(`${msg}: got ${String(actual)}`);
  const ak = Object.keys(actual).sort().join(",");
  const ek = Object.keys(expected).sort().join(",");
  if (ak !== ek) fail(`${msg}: keys [${ak}] != [${ek}]`);
  for (const k of Object.keys(expected)) {
    const a = actual[k], e = expected[k];
    if (e && typeof e === "object") assert_object_equals(a, e, `${msg}.${k}`);
    else assert_equals(a, e, `${msg}.${k}`);
  }
}
