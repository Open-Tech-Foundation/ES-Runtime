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
  // An asymmetric matcher stands where a value would: `expect.any(Number)`
  // inside an expected object is a *predicate*, not something to compare with.
  // Checked before anything else so it works at any depth — which is the only
  // reason to have them, since a top-level one could be its own assertion.
  if (isMatcher(b)) return b.matches(a);
  if (isMatcher(a)) return a.matches(b);
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

// ---------------------------------------------------------------------------
// expect
//
// The vocabulary the ecosystem writes tests in. `assertEquals(a, b)` and
// `expect(a).toEqual(b)` are the same assertion and share the same comparison —
// this is a second spelling, not a second implementation, and the reason to have
// it is that a suite written for any other runner should run here unchanged.
//
// Mocks and fake timers are the other half of the vocabulary, and they are a
// subsystem rather than a matcher — they live under `mock` and `clock`, below.
// ---------------------------------------------------------------------------

const MATCHER = Symbol.for("runtime:test.asymmetric");

const isMatcher = (v) => v !== null && typeof v === "object" && v[MATCHER] === true;

const matcher = (label, matches) => ({ [MATCHER]: true, label, matches });

function fail(actual, expected, negated, verb) {
  throw new Error(
    negated
      ? `expected ${show(actual)} not to ${verb} ${show(expected)}`
      : `expected ${show(actual)} to ${verb} ${show(expected)}`,
  );
}

// Every matcher is written as "does it hold?", and negation is applied in one
// place. Written the other way — a `not` object with its own inverted
// implementations — is how a suite ends up with a matcher whose negation does
// not mean what it says.
function check(held, negated, report) {
  if (held !== negated) return;
  report();
}

const lengthOf = (v) =>
  v === null || v === undefined
    ? undefined
    : typeof v.length === "number"
      ? v.length
      : typeof v.size === "number"
        ? v.size
        : undefined;

function contains(actual, wanted) {
  if (typeof actual === "string") {
    return typeof wanted === "string" && actual.includes(wanted);
  }
  if (actual instanceof Set || actual instanceof Map) {
    return actual.has(wanted);
  }
  if (actual !== null && typeof actual === "object" && typeof actual.length === "number") {
    return Array.prototype.some.call(actual, (v) => Object.is(v, wanted) || v === wanted);
  }
  return false;
}

// `toMatchObject`: every key the *expectation* names, compared structurally, and
// nothing said about the keys it does not name.
function matchesObject(actual, expected, seen) {
  if (isMatcher(expected)) return expected.matches(actual);
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length !== expected.length) return false;
    return expected.every((want, i) => matchesObject(actual[i], want, seen));
  }
  if (expected === null || typeof expected !== "object") return equal(actual, expected, seen);
  if (actual === null || typeof actual !== "object") return false;
  return Object.keys(expected).every(
    (key) =>
      Object.prototype.hasOwnProperty.call(actual, key) &&
      matchesObject(actual[key], expected[key], seen),
  );
}

function property(actual, path) {
  const parts = Array.isArray(path) ? path : String(path).split(".");
  let at = actual;
  for (const part of parts) {
    if (at === null || at === undefined) return { found: false };
    if (!(part in Object(at))) return { found: false };
    at = at[part];
  }
  return { found: true, value: at };
}

async function threw(fn) {
  try {
    const result = typeof fn === "function" ? fn() : fn;
    if (result && typeof result.then === "function") await result;
  } catch (err) {
    return { caught: true, err };
  }
  return { caught: false };
}

/// A mock's record, or a complaint that this is not a mock at all.
function recordOf(value, matcher) {
  if (!isMock(value)) {
    throw new TypeError(
      `expect(...).${matcher} needs a mock — mock.fn() or mock.spyOn(), and ` +
        `${show(value)} is neither`,
    );
  }
  return value.mock;
}

const callsOf = (value, matcher) => recordOf(value, matcher).calls;
const resultsOf = (value, matcher) => recordOf(value, matcher).results;

/// The calls a mock has seen, short enough to read in a failure.
const showCalls = (calls) =>
  calls.length === 0 ? "not called" : calls.map((call) => show(call)).join(", ");

/// A failure about a mock, which names it: a suite with six spies all reporting
/// "expected [Function] to have been called" says nothing about which one.
function called(mock, what, expected, negated) {
  const name = isMock(mock) ? mock.getMockName() : "the function";
  throw new Error(
    expected === undefined
      ? `expected ${name} ${what}`
      : negated
        ? `expected ${name} not to have been called with ${show(expected)}, and it was: ${what}`
        : `expected ${name} to have been called with ${show(expected)}, and it was ${what}`,
  );
}

function throwsSync(fn) {
  try {
    fn();
  } catch (err) {
    return { caught: true, err };
  }
  return { caught: false };
}

function expectation(actual, negated) {
  const it = {
    toBe(expected) {
      check(Object.is(actual, expected), negated, () => fail(actual, expected, negated, "be"));
    },
    toEqual(expected) {
      check(equal(actual, expected, []), negated, () => fail(actual, expected, negated, "equal"));
    },
    // The same comparison. Jest's stricter variant also distinguishes a missing
    // key from an undefined one and compares classes; saying so is better than
    // implying a strictness this does not have.
    toStrictEqual(expected) {
      it.toEqual(expected);
    },
    toBeTruthy() {
      check(Boolean(actual), negated, () =>
        fail(actual, "truthy", negated, "be"),
      );
    },
    toBeFalsy() {
      check(!actual, negated, () => fail(actual, "falsy", negated, "be"));
    },
    toBeNull() {
      check(actual === null, negated, () => fail(actual, null, negated, "be"));
    },
    toBeUndefined() {
      check(actual === undefined, negated, () => fail(actual, undefined, negated, "be"));
    },
    toBeDefined() {
      check(actual !== undefined, negated, () => fail(actual, "defined", negated, "be"));
    },
    toBeNaN() {
      check(Number.isNaN(actual), negated, () => fail(actual, NaN, negated, "be"));
    },
    toBeInstanceOf(constructor) {
      check(actual instanceof constructor, negated, () =>
        fail(actual, constructor?.name ?? constructor, negated, "be an instance of"),
      );
    },
    toBeTypeOf(type) {
      check(typeof actual === type, negated, () => fail(actual, type, negated, "be of type"));
    },
    toContain(wanted) {
      check(contains(actual, wanted), negated, () => fail(actual, wanted, negated, "contain"));
    },
    toContainEqual(wanted) {
      const held =
        actual !== null &&
        typeof actual === "object" &&
        Array.prototype.some.call(actual, (v) => equal(v, wanted, []));
      check(held, negated, () => fail(actual, wanted, negated, "contain an equal"));
    },
    toHaveLength(length) {
      check(lengthOf(actual) === length, negated, () =>
        fail(lengthOf(actual), length, negated, "have a length of"),
      );
    },
    toHaveProperty(path, ...value) {
      const found = property(actual, path);
      const held = value.length === 0 ? found.found : found.found && equal(found.value, value[0], []);
      check(held, negated, () =>
        fail(actual, value.length === 0 ? path : `${path} = ${show(value[0])}`, negated, "have"),
      );
    },
    toMatch(pattern) {
      const text = String(actual);
      const held = pattern instanceof RegExp ? pattern.test(text) : text.includes(String(pattern));
      check(held, negated, () => fail(actual, pattern, negated, "match"));
    },
    toMatchObject(expected) {
      check(matchesObject(actual, expected, []), negated, () =>
        fail(actual, expected, negated, "match"),
      );
    },
    toBeGreaterThan(n) {
      check(actual > n, negated, () => fail(actual, n, negated, "be greater than"));
    },
    toBeGreaterThanOrEqual(n) {
      check(actual >= n, negated, () =>
        fail(actual, n, negated, "be greater than or equal to"),
      );
    },
    toBeLessThan(n) {
      check(actual < n, negated, () => fail(actual, n, negated, "be less than"));
    },
    toBeLessThanOrEqual(n) {
      check(actual <= n, negated, () => fail(actual, n, negated, "be less than or equal to"));
    },
    // Two digits by default, as everywhere else this is spelled: the point of
    // the matcher is floating-point noise, not a tolerance anybody remembers.
    toBeCloseTo(n, digits = 2) {
      const held = Math.abs(actual - n) < 10 ** -digits / 2;
      check(held, negated, () => fail(actual, n, negated, `be close to (${digits} digits)`));
    },
    // --- what a mock was asked ---
    //
    // Every one of these needs `actual` to be a mock, and says so rather than
    // reporting that `undefined` is not what was expected: a matcher applied to
    // a plain function is a mistake in the test, not a failing assertion.
    toHaveBeenCalled() {
      const calls = callsOf(actual, "toHaveBeenCalled");
      check(calls.length > 0, negated, () =>
        called(actual, negated ? `to have been called ${calls.length} time(s)` : "never to have been called"),
      );
    },
    toHaveBeenCalledTimes(n) {
      const calls = callsOf(actual, "toHaveBeenCalledTimes");
      check(calls.length === n, negated, () =>
        fail(calls.length, n, negated, "have been called this many times:"),
      );
    },
    toHaveBeenCalledWith(...args) {
      const calls = callsOf(actual, "toHaveBeenCalledWith");
      check(calls.some((call) => equal(call, args, [])), negated, () =>
        called(actual, `called with ${showCalls(calls)}`, args, negated),
      );
    },
    toHaveBeenLastCalledWith(...args) {
      const calls = callsOf(actual, "toHaveBeenLastCalledWith");
      const last = calls.at(-1);
      check(calls.length > 0 && equal(last, args, []), negated, () =>
        called(actual, `last called with ${show(last)}`, args, negated),
      );
    },
    // 1-based, as everywhere this matcher is spelled: the first call is 1.
    toHaveBeenNthCalledWith(n, ...args) {
      const calls = callsOf(actual, "toHaveBeenNthCalledWith");
      const call = calls[n - 1];
      check(n >= 1 && n <= calls.length && equal(call, args, []), negated, () =>
        called(actual, `call ${n} was ${show(call)}`, args, negated),
      );
    },
    toHaveReturned() {
      const results = resultsOf(actual, "toHaveReturned");
      check(results.some((r) => r.type === "return"), negated, () =>
        called(actual, negated ? "to have returned" : "never to have returned without throwing"),
      );
    },
    toHaveReturnedTimes(n) {
      const results = resultsOf(actual, "toHaveReturnedTimes");
      const returned = results.filter((r) => r.type === "return").length;
      check(returned === n, negated, () =>
        fail(returned, n, negated, "have returned this many times:"),
      );
    },
    toHaveReturnedWith(value) {
      const results = resultsOf(actual, "toHaveReturnedWith");
      const held = results.some((r) => r.type === "return" && equal(r.value, value, []));
      check(held, negated, () => fail(actual, value, negated, "have returned"));
    },
    toHaveLastReturnedWith(value) {
      const results = resultsOf(actual, "toHaveLastReturnedWith");
      const last = results.at(-1);
      const held = last?.type === "return" && equal(last.value, value, []);
      check(held, negated, () => fail(last?.value, value, negated, "have last returned"));
    },
    toHaveNthReturnedWith(n, value) {
      const results = resultsOf(actual, "toHaveNthReturnedWith");
      const at = results[n - 1];
      const held = at?.type === "return" && equal(at.value, value, []);
      check(held, negated, () => fail(at?.value, value, negated, `have returned on call ${n}:`));
    },
    toThrow(want) {
      if (typeof actual !== "function") {
        throw new TypeError("expect(...).toThrow needs a function to call");
      }
      const outcome = throwsSync(actual);
      if (negated) {
        if (outcome.caught && matches(outcome.err, want)) {
          throw new Error(
            `expected it not to throw${want === undefined ? "" : ` ${showExpected(want)}`}` +
              `, and it threw ${showError(outcome.err)}`,
          );
        }
        return;
      }
      if (!outcome.caught) neverThrew(want, "", "throw", "");
      checkThrew(outcome.err, want, "", "throw", "");
    },
  };
  it.toThrowError = it.toThrow;
  // The shorter spellings, which are the same matchers under the names jest
  // gave them first. Aliases rather than copies: two implementations of one
  // assertion is how they end up disagreeing.
  it.toBeCalled = it.toHaveBeenCalled;
  it.toBeCalledTimes = it.toHaveBeenCalledTimes;
  it.toBeCalledWith = it.toHaveBeenCalledWith;
  it.lastCalledWith = it.toHaveBeenLastCalledWith;
  it.nthCalledWith = it.toHaveBeenNthCalledWith;
  it.toReturn = it.toHaveReturned;
  it.toReturnTimes = it.toHaveReturnedTimes;
  it.toReturnWith = it.toHaveReturnedWith;
  it.lastReturnedWith = it.toHaveLastReturnedWith;
  it.nthReturnedWith = it.toHaveNthReturnedWith;
  return it;
}

// `await expect(promise).resolves.toEqual(x)` — the promise is settled first and
// the matcher runs on what came out of it, so a rejection is reported as one
// rather than as a mismatched Promise object.
function awaited(promise, negated, wantResolved) {
  const handler = {
    get(_target, name) {
      // `.resolves.not.toBe(x)` — negation reached through the proxy, which
      // otherwise answers every name with a matcher and would hand back an
      // async function called `not`.
      if (name === "not") return awaited(promise, !negated, wantResolved);
      return async (...args) => {
        const outcome = await threw(promise);
        if (wantResolved && outcome.caught) {
          throw new Error(`expected it to resolve, and it rejected: ${showError(outcome.err)}`);
        }
        if (!wantResolved && !outcome.caught) {
          throw new Error("expected it to reject, and it resolved");
        }
        if (!wantResolved) {
          // `rejects.toThrow(...)` asserts about the error itself, and every
          // other matcher asserts about it as a value.
          const err = outcome.err;
          if (name === "toThrow" || name === "toThrowError") {
            if (negated) {
              if (matches(err, args[0])) {
                throw new Error(
                  `expected it not to reject with ${showExpected(args[0])}` +
                    `, and it did: ${showError(err)}`,
                );
              }
              return;
            }
            checkThrew(err, args[0], "", "reject", "with ");
            return;
          }
          expectation(err, negated)[name](...args);
          return;
        }
        const value = await promise;
        expectation(value, negated)[name](...args);
      };
    },
  };
  return new Proxy({}, handler);
}

function expect(actual) {
  const it = expectation(actual, false);
  it.not = expectation(actual, true);
  it.resolves = awaited(actual, false, true);
  it.rejects = awaited(actual, false, false);
  it.not.resolves = awaited(actual, true, true);
  it.not.rejects = awaited(actual, true, false);
  return it;
}

// The asymmetric matchers: a value that says what it will accept, usable
// wherever a value goes — including several levels inside an expected object,
// which is the case that cannot be written as an assertion of its own.
expect.anything = () =>
  matcher("anything", (v) => v !== null && v !== undefined);
expect.any = (constructor) =>
  matcher(`any(${constructor?.name ?? constructor})`, (v) => {
    if (constructor === String) return typeof v === "string" || v instanceof String;
    if (constructor === Number) return typeof v === "number" || v instanceof Number;
    if (constructor === Boolean) return typeof v === "boolean" || v instanceof Boolean;
    if (constructor === BigInt) return typeof v === "bigint";
    if (constructor === Symbol) return typeof v === "symbol";
    if (constructor === Function) return typeof v === "function";
    return v instanceof constructor;
  });
expect.stringContaining = (part) =>
  matcher(`stringContaining(${part})`, (v) => typeof v === "string" && v.includes(part));
expect.stringMatching = (pattern) =>
  matcher(`stringMatching(${pattern})`, (v) =>
    typeof v === "string" && (pattern instanceof RegExp ? pattern.test(v) : v.includes(pattern)),
  );
expect.arrayContaining = (wanted) =>
  matcher("arrayContaining", (v) =>
    Array.isArray(v) && wanted.every((want) => v.some((have) => equal(have, want, []))),
  );
expect.objectContaining = (wanted) =>
  matcher("objectContaining", (v) => matchesObject(v, wanted, []));


// ---------------------------------------------------------------------------
// mock, clock — standing in for a function, and for time
//
// A test asserts about what a function *did*, and about code that waits.
// Neither can be written as a matcher: a mock is a function with a record
// attached, and a fake clock replaces the timers a program schedules on.
//
// Imported like everything else — `import { mock, clock } from "runtime:test"`.
// Two namespaces rather than one, because they are two subsystems and naming
// them separately is what lets each verb be short: `clock.advance(100)` says
// what moved, where a single shared object forces `advanceTimersByTime`.
//
// The *methods* on a mock keep the names the ecosystem gave them —
// `mockReturnValue`, `mockClear`, `mock.calls` — for the reason `expect` exists
// here at all: they are the vocabulary the matchers read, and a suite written
// against another runner should need an import line rather than a rewrite.
//
// **The clock is the part with teeth.** `clock.freeze()` swaps `setTimeout`,
// `setInterval`, their cancels, and `Date` on `globalThis` — for the whole
// process, not for one test — and everything scheduled through them then moves
// only when the test says so. Those are standards-defined names being replaced
// at the test's own explicit request, which is the opposite of the runtime
// handing out a vocabulary; nothing here is ambient, and nothing is installed
// unless a file asks for it by importing it.
//
// It is safe for one further reason: a test file is a process (see
// [`crate::test`]), so the swap cannot reach the next file. The runner itself
// never schedules on a timer — it drains on microtasks — so a file that freezes
// the clock and forgets to release it cannot wedge the report.
// ---------------------------------------------------------------------------

const MOCK = Symbol.for("runtime:test.mock");
const RESTORE = Symbol.for("runtime:test.restore");

const isMock = (v) => typeof v === "function" && v[MOCK] === true;

// Every mock made in this file, so `restoreAll` can mean it. Strong
// references: the process is one test file, and it ends.
const made = new Set();

/// A function that records what it was called with, and answers however it was
/// told to.
function mockFn(implementation) {
  const once = [];
  let impl = implementation;
  let named = implementation?.name || "the mock";
  let restore = null;

  const blank = () => ({ calls: [], results: [], instances: [], lastCall: undefined });

  const fn = function (...args) {
    const record = fn.mock;
    record.calls.push(args);
    record.lastCall = args;
    if (new.target) record.instances.push(this);
    const use = once.length > 0 ? once.shift() : impl;
    if (!use) {
      record.results.push({ type: "return", value: undefined });
      return undefined;
    }
    try {
      const value = Reflect.apply(use, this, args);
      record.results.push({ type: "return", value });
      return value;
    } catch (err) {
      // Recorded *and* rethrown: a mock that swallowed the throw would send the
      // code under test down a path it does not take in production.
      fn.mock.results.push({ type: "throw", value: err });
      throw err;
    }
  };

  Object.defineProperty(fn, MOCK, { value: true });
  fn.mock = blank();

  fn.mockImplementation = (f) => ((impl = f), fn);
  fn.mockImplementationOnce = (f) => (once.push(f), fn);
  fn.mockReturnValue = (v) => fn.mockImplementation(() => v);
  fn.mockReturnValueOnce = (v) => fn.mockImplementationOnce(() => v);
  fn.mockReturnThis = () =>
    fn.mockImplementation(function () {
      return this;
    });
  fn.mockResolvedValue = (v) => fn.mockImplementation(() => Promise.resolve(v));
  fn.mockResolvedValueOnce = (v) => fn.mockImplementationOnce(() => Promise.resolve(v));
  fn.mockRejectedValue = (e) => fn.mockImplementation(() => Promise.reject(e));
  fn.mockRejectedValueOnce = (e) => fn.mockImplementationOnce(() => Promise.reject(e));

  // Three verbs, and the difference between them is what a test means by
  // "start again": forget the calls, forget how it was told to answer, or stop
  // standing in for the real thing altogether.
  fn.mockClear = () => ((fn.mock = blank()), fn);
  fn.mockReset = () => {
    fn.mockClear();
    once.length = 0;
    impl = implementation;
    return fn;
  };
  fn.mockRestore = () => {
    fn.mockReset();
    if (restore) restore();
    return fn;
  };

  fn.mockName = (name) => ((named = name), fn);
  fn.getMockName = () => named;
  // How `spyOn` says what putting the method back means. Not part of the API a
  // test writes against.
  fn[RESTORE] = (f) => {
    restore = f;
  };

  made.add(fn);
  return fn;
}

/// Replaces one method with a mock that still calls the original.
///
/// Calling through by default is the behaviour worth having: a spy is usually
/// installed to *watch* something work, and one that silently returned
/// `undefined` would change the result of every test that installed it.
/// `.mockImplementation(...)` is how a test says otherwise.
function spyOn(object, key) {
  if (object === null || (typeof object !== "object" && typeof object !== "function")) {
    throw new TypeError("mock.spyOn needs an object or a function to take the method from");
  }
  const own = Object.getOwnPropertyDescriptor(object, key);
  const original = own ? own.value : object[key];
  if (typeof original !== "function") {
    throw new TypeError(`mock.spyOn: ${String(key)} is not a method of that object`);
  }
  const spy = mockFn(function (...args) {
    return Reflect.apply(original, this, args);
  });
  spy.mockName(String(key));
  spy[RESTORE](() => {
    // An own property goes back exactly as it was; an inherited one is deleted
    // rather than written, so the prototype's method is found again.
    if (own) Object.defineProperty(object, key, own);
    else delete object[key];
  });
  Object.defineProperty(object, key, {
    value: spy,
    writable: true,
    configurable: true,
    enumerable: own ? own.enumerable : true,
  });
  return spy;
}

/// Globals a test replaced, and what was there before.
const stubbed = new Map();

const mock = {
  fn: mockFn,
  spyOn,
  is: isMock,
  // What a typed suite writes to tell the checker a real function is a mock.
  // There is nothing to satisfy at runtime, so it is the value itself.
  typed: (value) => value,

  /// Replaces a global for the duration of the file. `restoreAll` undoes it.
  global(name, value) {
    if (!stubbed.has(name)) {
      stubbed.set(name, Object.getOwnPropertyDescriptor(globalThis, name) ?? null);
    }
    Object.defineProperty(globalThis, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: true,
    });
    return mock;
  },

  clearAll() {
    for (const fn of made) fn.mockClear();
    return mock;
  },
  resetAll() {
    for (const fn of made) fn.mockReset();
    return mock;
  },
  /// Puts everything back: every spy's method, and every replaced global. The
  /// one call an `afterEach` needs, which is why globals are restored here
  /// rather than by a second verb a test can forget.
  restoreAll() {
    for (const fn of made) fn.mockRestore();
    for (const [name, descriptor] of stubbed) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
    stubbed.clear();
    return mock;
  },
};

// --- the clock ---

/// The installed fake clock, or `null` while time is real.
let frozen = null;

/// How many timers one `runAll` will fire before it decides the queue is not
/// going to end. An interval, or a timeout that reschedules itself, never
/// drains — and a test that hangs teaches nothing.
const RUNAWAY = 10_000;

/// The clock the test drives, or a complaint that time is still real.
function ticking(verb) {
  if (!frozen) {
    throw new Error(`clock.${verb} needs the frozen clock — call clock.freeze() first`);
  }
  return frozen;
}

/// The timer that comes next, if it is due by `limit`.
///
/// Ties are broken by the order they were scheduled in, which is the order the
/// platform runs them in and the only one a test can reason about.
function due(state, limit) {
  let found = null;
  for (const timer of state.timers.values()) {
    if (timer.at > limit) continue;
    if (!found || timer.at < found.at || (timer.at === found.at && timer.id < found.id)) {
      found = timer;
    }
  }
  return found;
}

/// Runs one timer, having first decided whether it runs again.
///
/// Rescheduled before it is called, so an interval that cancels itself from
/// inside its own callback actually stops.
function fire(state, timer) {
  state.now = timer.at;
  if (timer.every) timer.at += timer.every;
  else state.timers.delete(timer.id);
  Reflect.apply(timer.callback, undefined, timer.args);
}

/// Whatever is pending on the microtask queue, run.
///
/// A real macrotask, not `await Promise.resolve()`: a promise chain of unknown
/// depth is only guaranteed to be finished once the queue has drained, and
/// draining it is exactly what yielding to a real timer does.
const settle = () =>
  new Promise((resolve) => Reflect.apply(frozen.real.setTimeout, globalThis, [resolve, 0]));

const clock = {
  /// Stops time. Optionally at a given moment — otherwise wherever it is now.
  freeze(at) {
    if (frozen) return clock;
    const real = {
      setTimeout: globalThis.setTimeout,
      clearTimeout: globalThis.clearTimeout,
      setInterval: globalThis.setInterval,
      clearInterval: globalThis.clearInterval,
      Date: globalThis.Date,
    };
    const state = { now: real.Date.now(), timers: new Map(), next: 1, real };
    frozen = state;
    if (at !== undefined) clock.setSystemTime(at);

    const add = (callback, delay, args, every) => {
      const id = state.next++;
      state.timers.set(id, {
        at: state.now + Math.max(0, Number(delay) || 0),
        callback,
        args,
        every,
        id,
      });
      return id;
    };
    globalThis.setTimeout = (callback, delay, ...args) => add(callback, delay, args, null);
    globalThis.setInterval = (callback, delay, ...args) =>
      add(callback, delay, args, Math.max(1, Number(delay) || 0));
    globalThis.clearTimeout = (id) => void state.timers.delete(id);
    globalThis.clearInterval = globalThis.clearTimeout;

    // `Date` moves with the clock rather than being frozen separately, because
    // the two are one question: code that waits almost always also asks what
    // time it is, and a stopped `setTimeout` beside a running `Date.now()`
    // describes a machine that does not exist.
    globalThis.Date = class Date extends real.Date {
      constructor(...args) {
        if (args.length === 0) super(state.now);
        else super(...args);
      }
      static now() {
        return state.now;
      }
    };
    return clock;
  },

  /// Starts it again, and puts the real timers back.
  release() {
    if (!frozen) return clock;
    const { real } = frozen;
    globalThis.setTimeout = real.setTimeout;
    globalThis.clearTimeout = real.clearTimeout;
    globalThis.setInterval = real.setInterval;
    globalThis.clearInterval = real.clearInterval;
    globalThis.Date = real.Date;
    frozen = null;
    return clock;
  },

  isFrozen: () => frozen !== null,

  /// Moves time forward, running whatever comes due on the way.
  advance(ms) {
    const state = ticking("advance");
    const target = state.now + Math.max(0, Number(ms) || 0);
    for (let fired = 0; ; fired += 1) {
      const timer = due(state, target);
      if (!timer) break;
      if (fired >= RUNAWAY) {
        throw new Error(`clock.advance: ${RUNAWAY} timers fired and the queue is not draining`);
      }
      fire(state, timer);
    }
    state.now = target;
    return clock;
  },

  /// `advance`, pausing after each callback so whatever it resolved gets to run.
  ///
  /// This is the one to reach for when the code under test `await`s. The
  /// synchronous form fires every callback with nothing in between, so a
  /// `sleep(10).then(...)` has been *resolved* but its continuation has not run
  /// — and the assertion after it sees the state from before.
  async advanceAsync(ms) {
    const state = ticking("advanceAsync");
    const target = state.now + Math.max(0, Number(ms) || 0);
    // Before looking, not only after: a continuation left over from an earlier
    // advance has not run yet, so the timer it is about to schedule is not in
    // the queue and a loop starting here would decide there was nothing to do.
    await settle();
    for (let fired = 0; ; fired += 1) {
      const timer = due(state, target);
      if (!timer) break;
      if (fired >= RUNAWAY) {
        throw new Error(
          `clock.advanceAsync: ${RUNAWAY} timers fired and the queue is not draining`,
        );
      }
      fire(state, timer);
      await settle();
    }
    state.now = target;
    await settle();
    return clock;
  },

  /// Jumps to whenever the next timer is due, and runs it.
  next() {
    const state = ticking("next");
    const timer = due(state, Number.POSITIVE_INFINITY);
    if (timer) fire(state, timer);
    return clock;
  },

  async nextAsync() {
    clock.next();
    await settle();
    return clock;
  },

  /// Runs the queue until it is empty.
  runAll() {
    const state = ticking("runAll");
    for (let fired = 0; state.timers.size > 0; fired += 1) {
      if (fired >= RUNAWAY) {
        throw new Error(`clock.runAll: ${RUNAWAY} timers fired and the queue is not draining`);
      }
      clock.next();
    }
    return clock;
  },

  async runAllAsync() {
    clock.runAll();
    await settle();
    return clock;
  },

  /// Only what is waiting *now* — an interval fires once rather than for ever.
  runPending() {
    const state = ticking("runPending");
    const waiting = [...state.timers.values()]
      .map((timer) => ({ timer, at: timer.at }))
      .sort((a, b) => a.at - b.at || a.timer.id - b.timer.id)
      .map((entry) => entry.timer);
    for (const timer of waiting) {
      if (state.timers.has(timer.id)) fire(state, timer);
    }
    return clock;
  },

  async runPendingAsync() {
    clock.runPending();
    await settle();
    return clock;
  },

  /// How many timers are waiting.
  pending: () => (frozen ? frozen.timers.size : 0),

  /// Drops them all without running any.
  clear() {
    if (frozen) frozen.timers.clear();
    return clock;
  },

  /// Where the frozen clock stands. A `Date`, a number of milliseconds, or a
  /// string the platform's `Date` can parse.
  setSystemTime(time) {
    const state = ticking("setSystemTime");
    state.now = typeof time === "string" ? state.real.Date.parse(time) : Number(time);
    return clock;
  },

  /// The real time, while the clock is frozen — for measuring how long
  /// something actually took.
  realNow: () => (frozen ? frozen.real.Date.now() : Date.now()),
};


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
  expect,
  mock,
  clock,
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
  expect,
  mock,
  clock,
};
