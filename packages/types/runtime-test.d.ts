declare module "runtime:test" {
  /**
   * Registers a test and starts it immediately.
   *
   * Tests are **not** queued: each one starts when `test()` is called, so a
   * test awaiting a timer does not hold up the next. `esdev` reports the tally
   * once the program is done.
   *
   * ```ts
   * import { test, assertEquals } from "runtime:test";
   *
   * test("adds", () => assertEquals(add(2, 3), 5));
   * test("fetches", async () => assertEquals((await get("/")).status, 200));
   * ```
   *
   * A test that never settles is reported as a **failure** — "the test never
   * finished" — rather than being left out of a green run.
   */
  export function test(name: string, fn: () => void | Promise<void>): void;

  /** Fails with `message` (or "assertion failed") unless `condition` is truthy. */
  export function assert(condition: unknown, message?: string): asserts condition;

  /**
   * Fails unless the two values are **structurally** equal.
   *
   * Not a `JSON.stringify` comparison: `BigInt` and `NaN` compare through
   * `Object.is`, typed arrays and `ArrayBuffer` byte by byte, `Map` and `Set`
   * by contents, `Date`/`RegExp`/`Error` by what identifies them, and objects
   * by their key *set* rather than key order. Cycles terminate.
   */
  export function assertEquals(actual: unknown, expected: unknown, message?: string): void;

  /**
   * What a thrown error must be: an error `name` or a substring of its message,
   * a `RegExp` tested against the message, or a constructor for an `instanceof`
   * check.
   */
  export type ErrorExpectation = string | RegExp | (new (...args: never[]) => Error);

  /**
   * Fails unless `fn` throws — and, when `want` is given, unless the error
   * matches it. `message` is the label printed on failure, and is the *third*
   * argument: the second is the expectation.
   */
  export function assertThrows(
    fn: () => unknown,
    want?: ErrorExpectation,
    message?: string,
  ): void;

  /** The async form: fails unless the promise rejects, and matches `want`. */
  export function assertRejects(
    fn: () => Promise<unknown>,
    want?: ErrorExpectation,
    message?: string,
  ): Promise<void>;

  const _default: {
    test: typeof test;
    assert: typeof assert;
    assertEquals: typeof assertEquals;
    assertThrows: typeof assertThrows;
    assertRejects: typeof assertRejects;
  };
  export default _default;
}
