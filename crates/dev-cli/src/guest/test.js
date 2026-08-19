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

const ops = globalThis.__ops;

// Cases waiting to run, in the order they were written.
const queue = [];

// The lifecycle hooks, each in registration order. Several of the same kind are
// allowed and all of them run: a helper module and the test file both have a
// right to a `beforeEach`, and the one that loaded second is not the only one.
const hooks = { beforeAll: [], afterAll: [], beforeEach: [], afterEach: [] };

// Whether a drain is already scheduled or running, so registering ten tests in
// a row schedules one.
let draining = false;
// `beforeAll` and `afterAll` are once per program, not once per drain.
let setUp = false;
let tornDown = false;
// What `beforeAll` threw, if it did: every case then fails with it rather than
// running against a fixture that was never built.
let setUpFailure = null;

// The detail a failure is reported with: the stack when there is one, because a
// failure is only actionable if it names the line that failed.
const detail = (err) => (err?.stack ? String(err.stack) : String(err));

function hook(kind) {
  return function register(fn) {
    if (typeof fn !== "function") {
      throw new TypeError(`${kind}(): needs a function to run`);
    }
    hooks[kind].push(fn);
  };
}

// Runs once before the first test, and once after the last.
//
// "After the last" is decided by the queue being empty, since a file does not
// announce that it has finished registering. A test registered after the queue
// has already drained still runs — it simply runs after `afterAll`, which is
// the only honest answer available without a declaration this API does not
// have.
const beforeAll = hook("beforeAll");
const afterAll = hook("afterAll");
// Runs around every test, including one that fails. `afterEach` is cleanup, so
// it runs whatever happened — a `beforeEach` that threw included.
const beforeEach = hook("beforeEach");
const afterEach = hook("afterEach");

// Registers a test. It runs when the ones before it have finished.
//
// The id comes back now, at registration, and that is what keeps the report
// complete: a case that never got to start because an earlier one never settled
// is a case the host already knows about, and it is reported as a failure
// rather than silently missing from a green run.
function test(name, fn) {
  if (typeof fn !== "function") {
    throw new TypeError(`test(${JSON.stringify(String(name))}): needs a function to run`);
  }
  queue.push({ id: ops.test_registered(String(name)), fn });
  schedule();
}

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
    if (!setUp) {
      setUp = true;
      try {
        for (const fn of hooks.beforeAll) await fn();
      } catch (err) {
        setUpFailure = err;
      }
    }
    while (queue.length > 0) {
      await runCase(queue.shift());
    }
    if (!tornDown) {
      tornDown = true;
      for (const fn of hooks.afterAll) {
        try {
          await fn();
        } catch (err) {
          // Nothing is left to fail, so it is reported as a case of its own —
          // a teardown that threw is a broken suite, not a footnote.
          ops.test_finished(ops.test_registered("afterAll"), false, detail(err));
        }
      }
    }
  } finally {
    draining = false;
  }
}

async function runCase({ id, fn }) {
  ops.test_running(id);
  if (setUpFailure !== null) {
    ops.test_finished(id, false, `beforeAll failed, so this test never ran\n${detail(setUpFailure)}`);
    return;
  }
  let failure = null;
  try {
    for (const before of hooks.beforeEach) await before();
    await fn();
  } catch (err) {
    failure = err;
  }
  for (const after of hooks.afterEach) {
    try {
      await after();
    } catch (err) {
      // A cleanup that threw fails the case, unless the case had already
      // failed — the first failure is the one that explains the rest.
      failure ??= err;
    }
  }
  ops.test_finished(id, failure === null, failure === null ? "" : detail(failure));
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
  beforeAll,
  afterAll,
  beforeEach,
  afterEach,
  assert,
  assertEquals,
  assertThrows,
  assertRejects,
};
