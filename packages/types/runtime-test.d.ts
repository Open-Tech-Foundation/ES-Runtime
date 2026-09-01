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
    /**
     * A name with no body yet. Reported as **skipped**, never silently absent
     * — a to-do that vanished from the tally is the one missing case nobody
     * notices.
     */
    todo(name: string, fn?: () => void | Promise<void>): void;
    /** Registers the case only when the condition is false, and skips it otherwise. */
    skipIf(condition: unknown): TestFn;
    /** The mirror: registers it only when the condition holds. */
    runIf(condition: unknown): TestFn;
    /**
     * One case per row, named by substituting the row into the name.
     *
     * `%s`/`%d`/`%i`/`%f`/`%j`/`%o` take the next value positionally, `%#` is
     * the row's index, and `$key` takes a named property when the row is an
     * object. An array row is spread into the body's arguments, so the
     * parameters read like the table's header. A name that does not vary per
     * row gets an index appended, because six cases sharing one identity is a
     * report where a failure names none of them.
     *
     * ```ts
     * test.each([
     *   [1, 1, 2],
     *   [2, 3, 5],
     * ])("adds %d + %d = %d", (a, b, want) => expect(a + b).toBe(want));
     * ```
     */
    each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
  };

  /** What `test` is, for the conditional forms that hand it back. */
  export type TestFn = {
    (name: string, fn: () => void | Promise<void>): void;
    each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
  };

  /**
   * A table-driven registrar: `.each(rows)(name, body)`.
   *
   * A row that is an array is spread into the body's parameters; any other row
   * is passed as the single argument.
   */
  export type Each<Register> = <Row>(
    table: readonly Row[],
  ) => Row extends readonly unknown[]
    ? (name: string, fn: (...row: Row) => void | Promise<void>) => void
    : (name: string, fn: (row: Row) => void | Promise<void>) => void;

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
    /** A group planned and not written. Its name is reported as skipped. */
    todo(name: string, body?: () => void): void;
    /** Registers the group only when the condition is false. */
    skipIf(condition: unknown): (name: string, body: () => void) => void;
    /** …and only when it holds. */
    runIf(condition: unknown): (name: string, body: () => void) => void;
    /** One group per row — see {@link test.each}. */
    each: Each<(name: string, body: (...row: never[]) => void) => void>;
  };

  /**
   * `test`, under the name most of the ecosystem writes. The same function,
   * not a second one: two implementations of a registrar is how they end up
   * disagreeing about `.only`.
   */
  export const it: typeof test;

  /** `describe`, under vitest's name for it. */
  export const suite: typeof describe;

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

  /**
   * The matchers `expect(value)` answers with.
   *
   * The same comparisons the `assert*` functions use — this is a second
   * spelling, not a second implementation — so that a suite written against
   * another runner needs an import line rather than a rewrite.
   */
  export interface Matchers<T = unknown> {
    /** `Object.is` — reference identity, and `NaN` equals `NaN`. */
    toBe(expected: T): void;
    /** Structural equality, the same one {@link assertEquals} uses. */
    toEqual(expected: unknown): void;
    /** {@link Matchers.toEqual}. This runner draws no stricter distinction. */
    toStrictEqual(expected: unknown): void;
    toBeTruthy(): void;
    toBeFalsy(): void;
    toBeNull(): void;
    toBeUndefined(): void;
    toBeDefined(): void;
    toBeNaN(): void;
    toBeInstanceOf(constructor: new (...args: never[]) => unknown): void;
    toBeTypeOf(type: string): void;
    /** A member of an array, a substring, or a `Set`/`Map` key — by identity. */
    toContain(wanted: unknown): void;
    /** …and by structural equality. */
    toContainEqual(wanted: unknown): void;
    toHaveLength(length: number): void;
    /** `"a.b.c"` or `["a", "b"]`; with a value, that value must match too. */
    toHaveProperty(path: string | string[], value?: unknown): void;
    toMatch(pattern: string | RegExp): void;
    /** Every key in `expected` matches; keys outside it are not looked at. */
    toMatchObject(expected: object): void;
    toBeGreaterThan(n: number | bigint): void;
    toBeGreaterThanOrEqual(n: number | bigint): void;
    toBeLessThan(n: number | bigint): void;
    toBeLessThanOrEqual(n: number | bigint): void;
    /** Within half of `10 ** -digits`. Two digits by default. */
    toBeCloseTo(n: number, digits?: number): void;
    /** Calls the function and fails unless it throws — and matches `want`. */
    toThrow(want?: ErrorExpectation): void;
    /** {@link Matchers.toThrow}, under the name jest gave it. */
    toThrowError(want?: ErrorExpectation): void;

    /** Needs a {@link Mock}: `mock.fn()` or `mock.spyOn()`. */
    toHaveBeenCalled(): void;
    toHaveBeenCalledTimes(n: number): void;
    toHaveBeenCalledWith(...args: unknown[]): void;
    toHaveBeenLastCalledWith(...args: unknown[]): void;
    /** 1-based: the first call is `1`. */
    toHaveBeenNthCalledWith(n: number, ...args: unknown[]): void;
    /** Returned at least once **without throwing**. */
    toHaveReturned(): void;
    toHaveReturnedTimes(n: number): void;
    toHaveReturnedWith(value: unknown): void;
    toHaveLastReturnedWith(value: unknown): void;
    toHaveNthReturnedWith(n: number, value: unknown): void;

    /** The shorter spellings of the call matchers. Aliases, not variants. */
    toBeCalled(): void;
    toBeCalledTimes(n: number): void;
    toBeCalledWith(...args: unknown[]): void;
    lastCalledWith(...args: unknown[]): void;
    nthCalledWith(n: number, ...args: unknown[]): void;
    toReturn(): void;
    toReturnTimes(n: number): void;
    toReturnWith(value: unknown): void;
    lastReturnedWith(value: unknown): void;
    nthReturnedWith(n: number, value: unknown): void;
  }

  /** Every matcher, awaited — what `.resolves` and `.rejects` answer with. */
  export type AwaitedMatchers<T> = {
    [K in keyof Matchers<T>]: Matchers<T>[K] extends (...args: infer A) => void
      ? (...args: A) => Promise<void>
      : never;
  } & { not: AwaitedMatchers<T> };

  export interface Assertion<T = unknown> extends Matchers<T> {
    /** The same matchers, inverted. */
    not: Matchers<T>;
    /**
     * Settles the promise first and matches on what came out, so a rejection
     * is reported as one rather than as a mismatched `Promise`. `await` it.
     */
    resolves: AwaitedMatchers<Awaited<T>>;
    /**
     * The mirror: fails unless it rejects. `.rejects.toThrow(...)` asserts
     * about the error itself; every other matcher treats it as a value.
     */
    rejects: AwaitedMatchers<unknown>;
  }

  /**
   * The ecosystem's assertion vocabulary. `assertEquals(a, b)` and
   * `expect(a).toEqual(b)` are the same assertion.
   *
   * ```ts
   * import { test, expect } from "runtime:test";
   *
   * test("adds", () => expect(add(2, 3)).toBe(5));
   * test("fetches", async () => {
   *   await expect(get("/")).resolves.toMatchObject({ status: 200 });
   * });
   * ```
   *
   * The static members are the **asymmetric** matchers: a value that says what
   * it will accept, usable wherever a value goes — including several levels
   * inside an expected object, which is the case that cannot be written as an
   * assertion of its own.
   */
  export const expect: {
    <T>(actual: T): Assertion<T>;
    /** Anything but `null` or `undefined`. */
    anything(): any;
    /** `expect.any(Number)` — matches by constructor, primitives included. */
    any(constructor: unknown): any;
    stringContaining(part: string): any;
    stringMatching(pattern: string | RegExp): any;
    /** An array holding at least these, in any order. */
    arrayContaining(wanted: unknown[]): any;
    /** An object matching at least these keys. */
    objectContaining(wanted: object): any;
  };

  /** What a {@link Mock} remembers. */
  export interface MockRecord<A extends unknown[], R> {
    /** The arguments of every call, in order. */
    calls: A[];
    /** What each call did — returned a value, or threw one. */
    results: Array<{ type: "return"; value: R } | { type: "throw"; value: unknown }>;
    /** `this` for each call made with `new`. */
    instances: unknown[];
    /** The arguments of the most recent call, or `undefined`. */
    lastCall: A | undefined;
  }

  /**
   * A function that records what it was called with, and answers however it
   * was told to. The method names are the ecosystem's, because they are the
   * vocabulary the matchers read.
   */
  export interface Mock<A extends unknown[] = any[], R = any> {
    (...args: A): R;
    /** The record. Cleared by {@link Mock.mockClear}. */
    mock: MockRecord<A, R>;

    mockImplementation(fn: (...args: A) => R): this;
    /** Used for the next call only. Queued: several may be set. */
    mockImplementationOnce(fn: (...args: A) => R): this;
    mockReturnValue(value: R): this;
    mockReturnValueOnce(value: R): this;
    mockReturnThis(): this;
    mockResolvedValue(value: Awaited<R>): this;
    mockResolvedValueOnce(value: Awaited<R>): this;
    mockRejectedValue(error: unknown): this;
    mockRejectedValueOnce(error: unknown): this;

    /** Forgets the calls. */
    mockClear(): this;
    /** …and how it was told to answer, back to what it was created with. */
    mockReset(): this;
    /** …and, for a spy, puts the original method back. */
    mockRestore(): this;

    /** Names it, so a failure says which mock. */
    mockName(name: string): this;
    getMockName(): string;
  }

  /**
   * Functions that stand in for real ones.
   *
   * ```ts
   * import { test, expect, mock } from "runtime:test";
   *
   * test("retries", async () => {
   *   const send = mock.fn().mockRejectedValueOnce(new Error("nope"));
   *   await deliver(send);
   *   expect(send).toHaveBeenCalledTimes(2);
   * });
   * ```
   */
  export const mock: {
    /** A recording function, answering with `implementation` if given. */
    fn<A extends unknown[] = any[], R = any>(implementation?: (...args: A) => R): Mock<A, R>;
    /**
     * Replaces one method with a mock that **still calls the original** — a
     * spy is usually installed to watch something work.
     * `.mockImplementation(...)` is how a test says otherwise, and
     * `.mockRestore()` puts the property back exactly as it was.
     */
    spyOn<T extends object, K extends keyof T>(object: T, key: K): Mock;
    /** Whether a value is one of these. */
    is(value: unknown): boolean;
    /** Identity — for telling a type checker that a real function is a mock. */
    typed<T>(value: T): T;
    /** Replaces a global for the file. Undone by {@link mock.restoreAll}. */
    global(name: string, value: unknown): typeof mock;
    /** Forgets every mock's calls. */
    clearAll(): typeof mock;
    /** …and how each was told to answer. */
    resetAll(): typeof mock;
    /** Puts it all back: every spy's method, and every replaced global. */
    restoreAll(): typeof mock;
  };

  /**
   * Time, stopped.
   *
   * `freeze()` replaces `setTimeout`, `setInterval`, their cancels and `Date`
   * on `globalThis`, so everything scheduled through them moves only when the
   * test says so. It is safe because a test file is a **process**: the swap
   * cannot reach the next file, and the runner drains on microtasks rather
   * than timers, so a file that forgets {@link clock.release} still reports.
   *
   * ```ts
   * import { test, expect, mock, clock } from "runtime:test";
   *
   * test("gives up after a minute", async () => {
   *   clock.freeze();
   *   const gone = mock.fn();
   *   waitFor(gone, 60_000);
   *   await clock.advanceAsync(60_000);
   *   expect(gone).toHaveBeenCalled();
   *   clock.release();
   * });
   * ```
   */
  export const clock: {
    /** Stops time — at `at`, or wherever it is now. */
    freeze(at?: Date | number | string): typeof clock;
    /** Starts it again, and puts the real timers back. */
    release(): typeof clock;
    isFrozen(): boolean;
    /** Moves forward, running whatever comes due on the way. */
    advance(ms: number): typeof clock;
    /**
     * …pausing after each callback so whatever it resolved gets to run. The
     * one to use when the code under test `await`s: the synchronous form
     * resolves a promise but returns before anything waiting on it has run.
     */
    advanceAsync(ms: number): Promise<typeof clock>;
    /** Jumps to whenever the next timer is due, and runs it. */
    next(): typeof clock;
    nextAsync(): Promise<typeof clock>;
    /** Runs the queue until it is empty, or refuses one that never drains. */
    runAll(): typeof clock;
    runAllAsync(): Promise<typeof clock>;
    /** Only what is waiting now — an interval fires once, not for ever. */
    runPending(): typeof clock;
    runPendingAsync(): Promise<typeof clock>;
    /** How many timers are waiting. */
    pending(): number;
    /** Drops them all without running any. */
    clear(): typeof clock;
    /** Where the frozen clock stands. */
    setSystemTime(time: Date | number | string): typeof clock;
    /** The real time, while the clock is frozen. */
    realNow(): number;
  };

  const _default: {
    test: typeof test;
    it: typeof it;
    describe: typeof describe;
    suite: typeof suite;
    beforeAll: typeof beforeAll;
    afterAll: typeof afterAll;
    beforeEach: typeof beforeEach;
    afterEach: typeof afterEach;
    assert: typeof assert;
    assertEquals: typeof assertEquals;
    assertThrows: typeof assertThrows;
    assertRejects: typeof assertRejects;
    expect: typeof expect;
    mock: typeof mock;
    clock: typeof clock;
  };
  export default _default;
}
