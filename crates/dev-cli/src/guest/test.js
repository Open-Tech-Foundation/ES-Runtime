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
// started and how it ended, and `esdev` prints the summary and decides the exit
// code once the program is done. That is what removes the epilogue that used to
// be appended too — so a test file is now, from the first byte to the last,
// exactly the file the developer wrote.

const ops = globalThis.__ops;

// Registers a test and starts it immediately.
//
// Not queued and not awaited: tests run concurrently, and one that awaits a
// timer does not hold up the next. Nothing here collects promises — the host
// knows a case is outstanding from the moment `test()` is called, and a case
// that never settles is reported as unfinished rather than quietly dropped.
function test(name, fn) {
  if (typeof fn !== "function") {
    throw new TypeError(`test(${JSON.stringify(String(name))}): needs a function to run`);
  }
  const id = ops.test_started(String(name));
  (async () => {
    try {
      await fn();
      ops.test_finished(id, true, "");
    } catch (err) {
      // The stack, when there is one: a failure is only actionable if it names
      // the line that failed.
      ops.test_finished(id, false, err?.stack ? String(err.stack) : String(err));
    }
  })();
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

export { test, assert, assertEquals, assertThrows, assertRejects };
export default { test, assert, assertEquals, assertThrows, assertRejects };
