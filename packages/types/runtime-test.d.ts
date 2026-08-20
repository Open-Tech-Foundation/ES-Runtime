declare module "runtime:test" {
  /**
   * Registers a test. It runs when the ones before it have finished.
   *
   * Cases run **one at a time**, in the order the file wrote them. A test that
   * awaits holds up the next, deliberately: two tests sharing a database, a
   * temp directory, a port or a module global cannot interleave, and there is a
   * "before" for {@link beforeEach} to happen in. `esdev` reports the tally
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
   * finished" — rather than being left out of a green run, and the cases behind
   * it are reported as never having started.
   */
  export const test: {
    (name: string, fn: () => void | Promise<void>): void;
    /**
     * Registers the case and reports it as **skipped** without running it —
     * counted in the tally rather than left out of it, because a green run
     * that quietly ran fewer tests than it printed is the failure this runner
     * is arranged against. The body may be left out.
     */
    skip(name: string, fn?: () => void | Promise<void>): void;
    /**
     * Runs this case and skips the rest — the one you are working on. The
     * cases held back are counted and named in the report, so a `.only` left
     * in a commit is visible rather than being a suite that got faster.
     */
    only(name: string, fn: () => void | Promise<void>): void;
  };

  /**
   * A group of tests: a name that composes into theirs (`"db > inserts > …"`),
   * and — the half that matters — a **scope**. A hook registered inside the
   * body belongs to the tests inside it, so a file that sets up a database for
   * six of its twenty cases is not setting it up for the other fourteen.
   *
   * ```ts
   * describe("db", () => {
   *   beforeAll(() => open());     // once, before this group's first test
   *   afterAll(() => close());     // once, after this group's last test
   *   beforeEach(() => reset());   // around this group's tests only
   *
   *   test("inserts", () => …);
   * });
   * ```
   *
   * The body **registers and returns**: it is not where awaiting belongs, and
   * an `async` one is refused rather than half-run, because only the part
   * before its first `await` would register in time.
   */
  export const describe: {
    (name: string, body: () => void): void;
    /** Skips every test in the group, and reports each as skipped. */
    skip(name: string, body: () => void): void;
    /** Runs this group and skips everything outside it. */
    only(name: string, body: () => void): void;
  };

  /** A lifecycle hook. Several of a kind may be registered; all of them run. */
  export type Hook = () => void | Promise<void>;

  /**
   * Runs once before the first test **of its scope** — the file, or the
   * {@link describe} it is written in. One that throws fails every test in
   * that scope rather than letting them run against a fixture that was never
   * built.
   */
  export function beforeAll(fn: Hook): void;

  /**
   * Runs once after the last test of its scope — which is the point at which
   * that scope has no cases left, since a file does not announce that it has
   * finished registering. An inner group's runs before the outer one that set
   * up what it is tearing down.
   */
  export function afterAll(fn: Hook): void;

  /**
   * Runs before every test in scope, outermost group first. One that throws
   * fails that test.
   */
  export function beforeEach(fn: Hook): void;

  /**
   * Runs after every test in scope, innermost group first, including one that
   * failed — it is cleanup, so it runs whatever happened. One that throws fails
   * the test unless the test had already failed.
   */
  export function afterEach(fn: Hook): void;

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
    describe: typeof describe;
    beforeAll: typeof beforeAll;
    afterAll: typeof afterAll;
    beforeEach: typeof beforeEach;
    afterEach: typeof afterEach;
    assert: typeof assert;
    assertEquals: typeof assertEquals;
    assertThrows: typeof assertThrows;
    assertRejects: typeof assertRejects;
  };
  export default _default;
}
