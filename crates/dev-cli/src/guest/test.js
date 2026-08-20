// runtime:test — the test API, as a module you import (esdev only).
//
//   import { test, assert, assertEquals, assertThrows } from "runtime:test";
//
//   test("adds", () => assertEquals(add(2, 3), 5));
//
// It used to be five globals, prepended to every test file's own source as a
// single physical line so that the file's line 1 stayed line 1. That worked,
// and it was wrong for three reasons that only an import fixes:
//
//   * **Ambient globals are what this runtime does not do.** Every other piece
//     of host functionality here is a `runtime:` module. A test file was the
//     one place a program was handed names it never asked for.
//   * **Only the entry got them.** The harness was injected into the file being
//     run, so a shared `test-helpers.ts` next to it could not use `assertEquals`
//     — the one place a test suite most wants to share code.
//   * **They had no types.** There was nothing to declare them in, so a `.ts`
//     test file referenced five undeclared names and `tsc --noEmit` failed on
//     a suite that ran perfectly.
//
// The runner keeps the results, not this module: `test()` tells the host a case
// exists and how it ended, and `esdev` prints the summary and decides the exit
// code once the program is done. That is what removes the epilogue that used to
// be appended too — so a test file is now, from the first byte to the last,
// exactly the file the developer wrote.
//
// # One at a time
//
// A test used to *start* where it was written: `test()` called the function and
// returned, so every async case in a file ran at once. It was cheap and it was
// wrong. Two tests that share a database, a temp directory, a port or a module
// global interleave, and the failure is a flake nobody can reproduce; a
// `beforeEach` cannot exist at all, because there is no "before" — the next
// test has already started. The 230 lines of scheduler every suite ended up
// writing to get around it were the evidence.
//
// So registration and execution are separate. `test()` appends to a queue, and
// the queue drains one case at a time, in the order the file wrote them, with
// the lifecycle hooks around each. A test that awaits does hold up the next —
// deliberately: that is what "one at a time" means, and it is what makes shared
// state usable.
//
// The host is told about a case when it is **registered**, not when it starts,
// so a case that never got to run because an earlier one hung is still in the
// report — as a failure that says exactly that.
//
// # Groups
//
// `describe()` is a name and a scope, and the scope is the half that matters: a
// `beforeEach` written inside one belongs to the tests inside it, and a file
// that sets up a database for six of its twenty cases should not be setting it
// up for the other fourteen. Without that, a group is a naming convention, and
// a naming convention is a thing a template string already does.
//
// The body runs **synchronously**, at once, and registers; it is not where
// awaiting belongs. An `async` one is refused rather than half-run, because
// half of it registers before the first `await` and the rest lands after the
// queue has already drained.
//
// # skip and only
//
// A skipped case is **reported as skipped**, not left out. This runner already
// treats a case that never finished as a failure rather than a silence, for the
// reason that decides this too: a green run that quietly ran fewer tests than it
// printed is the worst thing a test runner can do. `only` is the same statement
// from the other side — the cases it did not run are counted and said out loud,
// so a `.only` left in a commit is visible in the tally rather than being a
// suite that passes in a tenth of the time.

const ops = globalThis.__ops;

// Cases waiting to run, in the order they were written.
const queue = [];

// A group: a name, the hooks that belong to it, and what it is still waiting
// for. The file itself is one, with no name — which is what makes a hook
// written outside any `describe` the outermost scope rather than a special
// case.
function group(name, parent) {
  return {
    name,
    parent,
    // Each in registration order. Several of the same kind are allowed and all
    // of them run: a helper module and the test file both have a right to a
    // `beforeEach`, and the one that loaded second is not the only one.
    hooks: { beforeAll: [], afterAll: [], beforeEach: [], afterEach: [] },
    // Cases registered in it or in a group inside it, still to finish. What
    // decides when its `afterAll` runs.
    left: 0,
    opened: false,
    closed: false,
    // What its `beforeAll` threw, if it did: every case under it then fails
    // with that rather than running against a fixture that was never built.
    failure: null,
    skip: false,
    only: false,
  };
}

// The file's own scope, and the one being registered into right now.
const file = group("", null);
let current = file;
// Every group made, in the order they were made — so what is left open at the
// end is closed innermost first.
const groups = [file];

// Whether a drain is already scheduled or running, so registering ten tests in
// a row schedules one.
let draining = false;
// Whether anything anywhere asked to be the only thing that runs. Sticky: a
// `.only` registered after a drain has begun still speaks for the cases behind
// it, and un-deciding it later would run tests the file said not to.
let exclusive = false;

// The detail a failure is reported with: the stack when there is one, because a
// failure is only actionable if it names the line that failed.
const detail = (err) => (err?.stack ? String(err.stack) : String(err));

// The name a case is reported under: its groups, outermost first, then its own.
function label(scope, name) {
  const parts = [name];
  for (let at = scope; at && at.parent; at = at.parent) parts.unshift(at.name);
  return parts.join(" > ");
}

// Every hook of a kind that applies to a case, outermost group first — which is
// the order a `beforeEach` has to run in, and the reverse of an `afterEach`.
function around(scope, kind) {
  const out = [];
  for (let at = scope; at; at = at.parent) out.unshift(...at.hooks[kind]);
  return out;
}

// The `beforeAll` failure a case inherits: its own group's, or the nearest one
// outside it.
function broken(scope) {
  for (let at = scope; at; at = at.parent) if (at.failure !== null) return at.failure;
  return null;
}

function hook(kind) {
  return function register(fn) {
    if (typeof fn !== "function") {
      throw new TypeError(`${kind}(): needs a function to run`);
    }
    // The group being registered into, not the file: a `beforeEach` inside a
    // `describe` is that group's, and running it for the file's other tests is
    // the thing having groups at all is meant to stop.
    current.hooks[kind].push(fn);
  };
}

// Runs once before the first test **of its group**, and once after the last.
//
// At the top level of a file that is once per file. "After the last" is decided
// by the group having no cases left, since a file does not announce that it has
// finished registering — so a test registered after its group has drained runs
// after that group's `afterAll`, which is the only honest answer available
// without a declaration this API does not have.
const beforeAll = hook("beforeAll");
const afterAll = hook("afterAll");
// Runs around every test in scope, including one that fails. `afterEach` is
// cleanup, so it runs whatever happened — a `beforeEach` that threw included.
const beforeEach = hook("beforeEach");
const afterEach = hook("afterEach");

// A group of tests. The body registers and returns; it does not await.
function describe(name, body, mode) {
  if (typeof body !== "function") {
    throw new TypeError(
      `describe(${JSON.stringify(String(name))}): needs a function that registers the tests`,
    );
  }
  const made = group(String(name), current);
  // A group that is skipped skips everything in it, and one marked `only`
  // makes every case in it an `only` — which is what makes `describe.only`
  // mean the group rather than nothing.
  made.skip = mode === "skip" || current.skip;
  made.only = mode === "only" || current.only;
  if (made.only) exclusive = true;
  groups.push(made);
  const outer = current;
  current = made;
  try {
    const returned = body();
    if (returned !== null && typeof returned?.then === "function") {
      throw new TypeError(
        `describe(${JSON.stringify(String(name))}): the body registers tests and returns — ` +
          `it cannot be async, because only the part before its first await would register ` +
          `in time. Await inside a test, or in beforeAll.`,
      );
    }
  } finally {
    current = outer;
  }
}

// Registers a test. It runs when the ones before it have finished.
//
// The id comes back now, at registration, and that is what keeps the report
// complete: a case that never got to start because an earlier one never settled
// is a case the host already knows about, and it is reported as a failure
// rather than silently missing from a green run.
function test(name, fn, mode) {
  const skip = mode === "skip" || current.skip;
  if (!skip && typeof fn !== "function") {
    throw new TypeError(`test(${JSON.stringify(String(name))}): needs a function to run`);
  }
  const scope = current;
  const id = ops.test_registered(label(scope, String(name)));
  if (skip) {
    // Reported now and never queued: nothing about it runs, its group's
    // `beforeAll` included.
    ops.test_skipped(id, "");
    return;
  }
  const only = mode === "only" || scope.only;
  if (only) exclusive = true;
  for (let at = scope; at; at = at.parent) at.left += 1;
  queue.push({ id, fn, scope, only });
  schedule();
}

// `test.skip(...)` and `test.only(...)`, and the same pair on `describe`.
test.skip = (name, fn) => test(name, fn, "skip");
test.only = (name, fn) => test(name, fn, "only");
describe.skip = (name, body) => describe(name, body, "skip");
describe.only = (name, body) => describe(name, body, "only");

function schedule() {
  if (draining) return;
  draining = true;
  // A microtask, not a call: the rest of the file is still registering, and a
  // drain that started on the first `test()` would run case one before case two
  // existed. By the time microtasks run, the module body is done.
  queueMicrotask(() => {
    drain();
  });
}

async function drain() {
  try {
    while (queue.length > 0) {
      const next = queue.shift();
      // Something asked to be the only thing that runs, and this is not it.
      // Decided here rather than at registration, because whether a case is
      // the exception is not known until the file has finished registering.
      if (exclusive && !next.only) {
        ops.test_skipped(next.id, "only");
        await settled(next.scope);
        continue;
      }
      await runCase(next);
    }
    // Whatever is still open — a group whose last case has run leaves through
    // `settled` below, so this is the file's own scope, and any group a case
    // registered into after its own drain.
    for (const scope of [...groups].reverse()) await close(scope);
  } finally {
    draining = false;
  }
}

// Runs a group's `beforeAll`, and its enclosing groups' first. Once each.
async function open(scope) {
  if (scope.parent) await open(scope.parent);
  if (scope.opened) return;
  scope.opened = true;
  // An outer `beforeAll` failed, so this one does not run: it would be setting
  // up on top of something that was never built.
  if (broken(scope.parent) !== null) return;
  try {
    for (const fn of scope.hooks.beforeAll) await fn();
  } catch (err) {
    scope.failure = err;
  }
}

// Runs a group's `afterAll`, if its `beforeAll` ran.
async function close(scope) {
  if (!scope.opened || scope.closed) return;
  scope.closed = true;
  for (const fn of scope.hooks.afterAll) {
    try {
      await fn();
    } catch (err) {
      // Nothing is left to fail, so it is reported as a case of its own — a
      // teardown that threw is a broken suite, not a footnote.
      ops.test_finished(ops.test_registered(label(scope, "afterAll")), false, detail(err));
    }
  }
}

// One case is done. A group with nothing left is finished with, innermost
// first — so an inner `afterAll` runs before the outer one that set up what it
// is tearing down.
async function settled(scope) {
  for (let at = scope; at; at = at.parent) {
    at.left -= 1;
    if (at.left === 0) await close(at);
  }
}

async function runCase({ id, fn, scope }) {
  ops.test_running(id);
  await open(scope);
  const failed = broken(scope);
  if (failed !== null) {
    ops.test_finished(id, false, `beforeAll failed, so this test never ran\n${detail(failed)}`);
    await settled(scope);
    return;
  }
  let failure = null;
  try {
    for (const before of around(scope, "beforeEach")) await before();
    await fn();
  } catch (err) {
    failure = err;
  }
  for (const after of around(scope, "afterEach").reverse()) {
    try {
      await after();
    } catch (err) {
      // A cleanup that threw fails the case, unless the case had already
      // failed — the first failure is the one that explains the rest.
      failure ??= err;
    }
  }
  ops.test_finished(id, failure === null, failure === null ? "" : detail(failure));
  await settled(scope);
}

function assert(condition, message) {
  if (!condition) throw new Error(message || "assertion failed");
}

// Keys that carry a value. `{ a: 1, b: undefined }` and `{ a: 1 }` are the same
// object to anyone reading them, and an equality test that disagreed would fail
// on the difference between a field left out and a field set to nothing.
const definedKeys = (o) => Object.keys(o).filter((k) => o[k] !== undefined);

const sameBytes = (a, b) => {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
};

// Structural equality, deliberately not `JSON.stringify` on both sides.
//
// That is how this started, and it was wrong in a way that mattered on this
// runtime specifically: `JSON.stringify` *throws* on a BigInt, so the one
// assertion an int64 test most needs could not be written; a Uint8Array
// stringified to `{"0":1,"1":2}` instead of comparing as bytes; and object key
// order decided the result, which no equality test wants.
function equal(a, b, seen) {
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null || typeof a !== "object") return false;

  const tag = Object.prototype.toString.call(a);
  if (tag !== Object.prototype.toString.call(b)) return false;

  // Compared by what identifies them, not by their fields.
  if (a instanceof Date) return a.getTime() === b.getTime();
  if (a instanceof RegExp) return a.source === b.source && a.flags === b.flags;
  if (a instanceof Error) return a.name === b.name && a.message === b.message;

  // A pair already being compared is assumed equal: that is what makes a cyclic
  // structure terminate instead of blowing the stack.
  for (const pair of seen) if (pair[0] === a && pair[1] === b) return true;
  seen.push([a, b]);

  if (a instanceof ArrayBuffer) return sameBytes(new Uint8Array(a), new Uint8Array(b));
  if (ArrayBuffer.isView(a)) {
    return sameBytes(
      new Uint8Array(a.buffer, a.byteOffset, a.byteLength),
      new Uint8Array(b.buffer, b.byteOffset, b.byteLength),
    );
  }

  if (a instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [k, v] of a) {
      // The fast path: an identical key. Otherwise every entry has to be tried,
      // because two structurally equal keys are not the same object.
      if (b.has(k)) {
        if (!equal(v, b.get(k), seen)) return false;
        continue;
      }
      let found = false;
      for (const [k2, v2] of b) {
        if (equal(k, k2, seen) && equal(v, v2, seen)) {
          found = true;
          break;
        }
      }
      if (!found) return false;
    }
    return true;
  }

  if (a instanceof Set) {
    if (a.size !== b.size) return false;
    for (const v of a) {
      if (b.has(v)) continue;
      let found = false;
      for (const v2 of b) {
        if (equal(v, v2, seen)) {
          found = true;
          break;
        }
      }
      if (!found) return false;
    }
    return true;
  }

  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!equal(a[i], b[i], seen)) return false;
    return true;
  }

  const ka = definedKeys(a);
  const kb = definedKeys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
    if (!equal(a[k], b[k], seen)) return false;
  }
  return true;
}

// A value, for a failure message. Everything `JSON.stringify` refuses or
// mangles is handled first, because those are exactly the values a failing
// assertion is most often about.
function show(v) {
  if (typeof v === "bigint") return `${v}n`;
  if (v === undefined || typeof v === "symbol" || typeof v === "function") return String(v);
  try {
    const s = JSON.stringify(v, (_key, x) => {
      if (typeof x === "bigint") return `${x}n`;
      if (ArrayBuffer.isView(x) && !(x instanceof DataView)) return Array.from(x);
      if (x instanceof Map) return Array.from(x);
      if (x instanceof Set) return Array.from(x);
      return x;
    });
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
}

const showError = (e) =>
  e && e.name && e.message !== undefined ? `${e.name}: ${e.message}` : String(e);

const showExpected = (want) =>
  typeof want === "function" ? want.name || "the expected error" : String(want);

// What the second argument to assertThrows/assertRejects means: an error name or
// a substring of its message, a RegExp over the message, or a constructor for an
// instanceof check.
function matches(err, want) {
  if (want === undefined || want === null) return true;
  const message = err && err.message !== undefined ? String(err.message) : String(err);
  const name = err && err.name ? String(err.name) : "";
  if (typeof want === "string") return name === want || message.includes(want);
  if (want instanceof RegExp) return want.test(message) || want.test(showError(err));
  if (typeof want === "function") return err instanceof want;
  return false;
}

function checkThrew(err, want, message, verb, connective) {
  if (matches(err, want)) return;
  throw new Error(
    `${message ? `${message}: ` : ""}expected it to ${verb} ${connective}${showExpected(want)}` +
      `, got ${showError(err)}`,
  );
}

function neverThrew(want, message, verb, connective) {
  const expectation = want === undefined ? "" : ` ${connective}${showExpected(want)}`;
  throw new Error(
    `${message ? `${message}: ` : ""}expected it to ${verb}${expectation}, but it did not`,
  );
}

function assertEquals(actual, expected, message) {
  if (equal(actual, expected, [])) return;
  throw new Error(
    `${message ? `${message}: ` : ""}expected ${show(expected)}, got ${show(actual)}`,
  );
}

// The second argument is what the error must be, not a label. It used to be the
// message printed on failure, which made the natural thing to write —
// `assertThrows(fn, "TypeError")` — assert nothing at all: any throw passed.
function assertThrows(fn, want, message) {
  let threw;
  let caught = false;
  try {
    fn();
  } catch (err) {
    threw = err;
    caught = true;
  }
  if (!caught) neverThrew(want, message, "throw", "");
  checkThrew(threw, want, message, "throw", "");
}

async function assertRejects(fn, want, message) {
  let threw;
  let caught = false;
  try {
    await fn();
  } catch (err) {
    threw = err;
    caught = true;
  }
  if (!caught) neverThrew(want, message, "reject", "with ");
  checkThrew(threw, want, message, "reject", "with ");
}

export {
  test,
  describe,
  beforeAll,
  afterAll,
  beforeEach,
  afterEach,
  assert,
  assertEquals,
  assertThrows,
  assertRejects,
};
export default {
  test,
  describe,
  beforeAll,
  afterAll,
  beforeEach,
  afterEach,
  assert,
  assertEquals,
  assertThrows,
  assertRejects,
};
