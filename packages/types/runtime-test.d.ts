declare module "runtime:test" {
  /** A test's body. */
  export type TestBody = () => void | Promise<void>;

  /**
   * What a single test may ask for: `{ timeout, retry }`, or a bare number of
   * milliseconds for the timeout. Written after the body or before it.
   */
  export type TestOptions =
    | number
    | {
        /** Fail the test if its body has not settled within this many milliseconds. */
        timeout?: number;
        /** Run it again, up to this many more times, until it passes. */
        retry?: number;
        /** Run it this many more times after the first; it fails if any run fails. */
        repeats?: number;
      };

  /** The ways a test is registered: name, body, and optional options. */
  export type Register = {
    (name: string, fn: TestBody, options?: TestOptions): void;
    (name: string, options: TestOptions, fn: TestBody): void;
  };

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
  export const test: Register & {
    /**
     * Registers the case and reports it as **skipped** without running it —
     * counted in the tally rather than left out of it, because a green run
     * that quietly ran fewer tests than it printed is the failure this runner
     * is arranged against. The body may be left out.
     */
    skip: Register & ((name: string) => void);
    /**
     * Runs this case and skips the rest — the one you are working on. The
     * cases held back are counted and named in the report, so a `.only` left
     * in a commit is visible rather than being a suite that got faster.
     */
    only: Register;
    /**
     * A test that is known to fail. It passes while it fails, and fails once
     * it passes, so a fixed bug is noticed.
     */
    fails: Register & {
      each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
    };
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
  export type TestFn = Register & {
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

  /** Matches a versioned snapshot recorded for this test. */
  export function assertSnapshot(value: unknown, name?: string): void;

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
    /** Matches the versioned snapshot recorded for this test. */
    toMatchSnapshot(nameOrMatchers?: string | Record<string, unknown>): void;
    /** Matches exact text or bytes in this test's snapshot directory. */
    toMatchFileSnapshot(name: string): void;
    /** Calls the function and snapshots the error it throws. */
    toThrowErrorMatchingSnapshot(name?: string): void;
    /**
     * Matches the snapshot written in the call. With none yet, a local run
     * writes it into the source; `--ci` fails instead.
     */
    toMatchInlineSnapshot(snapshot?: string): void;
    toMatchInlineSnapshot(propertyMatchers: Record<string, unknown>, snapshot?: string): void;
    /** Calls the function and matches the error it throws against the snapshot written in the call. */
    toThrowErrorMatchingInlineSnapshot(snapshot?: string): void;
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
    /** The predicate returns something truthy for the value. */
    toSatisfy(predicate: (value: T) => unknown, message?: string): void;
    /** Structurally equal to one of the options. */
    toBeOneOf(options: readonly unknown[]): void;

    /**
     * DOM matchers. Each needs a DOM node and fails with a `TypeError` naming
     * the matcher otherwise — except `toBeInTheDocument`, which accepts `null`
     * so a query that found nothing can be asserted with `.not`.
     */
    toBeInTheDocument(): void;
    /** Connected, and neither it nor an ancestor is hidden, `display: none` or transparent. */
    toBeVisible(): void;
    /** No child nodes other than comments. */
    toBeEmptyDOMElement(): void;
    toContainElement(element: Node | null | undefined): void;
    /** The node's `outerHTML` contains this markup, as the parser would write it. */
    toContainHTML(html: string): void;
    /** `textContent`, with whitespace collapsed unless `normalizeWhitespace: false`. */
    toHaveTextContent(text: string | RegExp, options?: { normalizeWhitespace?: boolean }): void;
    /** With a value, the attribute must also equal it. */
    toHaveAttribute(name: string, value?: unknown): void;
    /** Every class named; `{ exact: true }` also forbids others. No names: any class. */
    toHaveClass(...names: Array<string | { exact?: boolean }>): void;
    /** A form control's value: a number for number and range inputs, an array for `<select multiple>`. */
    toHaveValue(value: unknown): void;
    /** A checkbox or radio, or an element with a checkable `role` and `aria-checked`. */
    toBeChecked(): void;
    /** Disabled itself, or inside a disabled `<fieldset>`. */
    toBeDisabled(): void;
    toBeEnabled(): void;
    /** `required`, or `aria-required="true"`. */
    toBeRequired(): void;
    toHaveFocus(): void;
    /** Each property as the node's computed style has it. Colours compute to `rgb()`. */
    toHaveStyle(css: string | Record<string, string | number>): void;

    /** Needs a {@link Mock}: `mock.fn()` or `mock.spyOn()`. */
    toHaveBeenCalled(): void;
    toHaveBeenCalledTimes(n: number): void;
    toHaveBeenCalledOnce(): void;
    toHaveBeenCalledWith(...args: unknown[]): void;
    toHaveBeenLastCalledWith(...args: unknown[]): void;
    /** 1-based: the first call is `1`. */
    toHaveBeenNthCalledWith(n: number, ...args: unknown[]): void;
    /**
     * Needs a {@link When} chain: every answer used — `times` of them, or at
     * least once for one without a limit. A chain with no answers never is.
     */
    toHaveBeenExhausted(): void;
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
    /** A number within `digits` decimal places of `n`. Two by default. */
    closeTo(n: number, digits?: number): any;
    /** The asymmetric matchers, inverted. */
    not: {
      stringContaining(part: string): any;
      stringMatching(pattern: string | RegExp): any;
      arrayContaining(wanted: unknown[]): any;
      objectContaining(wanted: object): any;
    } & AsymmetricMatchers;
    /**
     * A failed matcher is recorded and the test continues. It fails at the
     * end, listing every soft failure.
     */
    soft<T>(actual: T): Assertion<T>;
    /** The running test fails unless exactly `n` assertions ran in it. */
    assertions(n: number): void;
    /** The running test fails unless at least one assertion ran in it. */
    hasAssertions(): void;
    /** Fails where it is reached. */
    unreachable(message?: string): never;
    /**
     * Adds matchers. Each is called with the received value and its arguments,
     * and returns `{ pass, message }` (or a promise of one). Declare them for
     * TypeScript by augmenting {@link Matchers}, and {@link AsymmetricMatchers}
     * for use inside an expected value.
     */
    extend(matchers: Record<string, CustomMatcher>): void;
    /**
     * Calls `fn` until the matcher holds for what it returns, or `timeout`
     * passes (1000ms, checked every 50ms). Always `await` it.
     */
    poll<T>(fn: () => T | Promise<T>, options?: WaitOptions): AwaitedMatchers<Awaited<T>>;
  } & AsymmetricMatchers;

  /** How long `expect.poll` and {@link waitFor} keep trying, in milliseconds. */
  export interface WaitOptions {
    /** Give up after this long. 1000 by default. */
    timeout?: number;
    /** Wait this long between tries. 50 by default. */
    interval?: number;
  }

  /** What a custom matcher returns. */
  export interface MatcherResult {
    pass: boolean;
    message?: string | (() => string);
  }

  /** `this` inside a custom matcher. */
  export interface MatcherContext {
    /** Whether it was called through `.not`. */
    isNot: boolean;
    /** The structural equality `toEqual` uses. */
    equals(a: unknown, b: unknown): boolean;
    utils: {
      stringify(value: unknown): string;
      printReceived(value: unknown): string;
      printExpected(value: unknown): string;
    };
  }

  export type CustomMatcher = (
    this: MatcherContext,
    received: any,
    ...args: any[]
  ) => MatcherResult | Promise<MatcherResult>;

  /**
   * The asymmetric forms of matchers added with `expect.extend`. Augment it to
   * type them:
   *
   * ```ts
   * declare module "runtime:test" {
   *   interface Matchers<T> { toBeWithin(lo: number, hi: number): void }
   *   interface AsymmetricMatchers { toBeWithin(lo: number, hi: number): any }
   * }
   * ```
   */
  // biome-ignore lint/suspicious/noEmptyInterface: augmented by users
  export interface AsymmetricMatchers {}

  /**
   * Registers cleanup for the running test. Runs after its `afterEach` hooks,
   * newest first. Call it inside a test or its `beforeEach`.
   */
  export function onTestFinished(fn: () => void | Promise<void>): void;

  /** Runs only if the running test fails, and is given the failure. */
  export function onTestFailed(fn: (error: unknown) => void | Promise<void>): void;

  /**
   * Calls `fn` until it returns without throwing, or its promise resolves, and
   * returns what it returned. Gives up after `timeout` (1000ms, checked every
   * 50ms), throwing the last failure.
   */
  export function waitFor<T>(fn: () => T | Promise<T>, options?: WaitOptions): Promise<T>;

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
  export interface Mock<A extends unknown[] = any[], R = any> extends Disposable {
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
    /** Throws `error` from every call. */
    mockThrow(error: unknown): this;
    /** Throws `error` from the next call only. */
    mockThrowOnce(error: unknown): this;
    /** The implementation it answers with, or `undefined`. */
    getMockImplementation(): ((...args: A) => R) | undefined;
    /**
     * Answers with `fn` while `callback` runs — until its promise settles, when
     * it returns one.
     */
    withImplementation(fn: (...args: A) => R, callback: () => Promise<unknown>): Promise<void>;
    withImplementation(fn: (...args: A) => R, callback: () => unknown): void;

    /** Forgets the calls. */
    mockClear(): this;
    /** …and how it was told to answer, back to what it was created with. */
    mockReset(): this;
    /**
     * …and, for a spy, puts the original method back. `using spy = …` calls it
     * when the block ends.
     */
    mockRestore(): this;

    /** Names it, so a failure says which mock. */
    mockName(name: string): this;
    getMockName(): string;
  }

  /** What a mock does with arguments no answer matches. */
  export interface WhenOptions {
    /**
     * `"passthrough"` (the default) calls what the mock answered with before;
     * `"throw"` throws, naming the arguments; a function is called with them.
     */
    onUnmatched?: "passthrough" | "throw" | ((...args: any[]) => unknown);
  }

  /** How many calls an answer is for. Without `times`, every call. */
  export interface AnswerOptions {
    times?: number;
  }

  /**
   * A mock's answers by argument, from {@link mock.when}. The newest answer for
   * matching arguments is used first; one that has used up its `times` lets an
   * older one answer. Disposing it (`using`) puts back what the mock answered
   * with before.
   */
  export interface When<A extends unknown[] = any[], R = any> extends Disposable {
    /** The arguments the next answers are for: equal, or asymmetric matchers. */
    calledWith(...args: A): When<A, R>;
    thenReturn(value: R, options?: AnswerOptions): When<A, R>;
    thenReturnOnce(value: R): When<A, R>;
    thenThrow(error: unknown, options?: AnswerOptions): When<A, R>;
    thenThrowOnce(error: unknown): When<A, R>;
    thenResolve(value: Awaited<R>, options?: AnswerOptions): When<A, R>;
    thenResolveOnce(value: Awaited<R>): When<A, R>;
    thenReject(error: unknown, options?: AnswerOptions): When<A, R>;
    thenRejectOnce(error: unknown): When<A, R>;
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
    /** Watches a getter or setter while preserving the other accessor. */
    spyOn<T extends object, K extends keyof T>(object: T, key: K, accessType: "get" | "set"): Mock;
    /** Whether a value is one of these. */
    is(value: unknown): boolean;
    /** Identity — for telling a type checker that a real function is a mock. */
    typed<T>(value: T): T;
    /** Replaces a global for the file. Undone by {@link mock.restoreAll}. */
    global(name: string, value: unknown): typeof mock;
    /**
     * Sets a variable in `runtime:process`'s `env`; `undefined` removes it.
     * Undone by {@link mock.restoreAll}. Not in browser runs.
     */
    env(name: string, value: string | undefined): typeof mock;
    /** Answers by argument, replacing what `spy` answers with until disposed. */
    when<A extends unknown[], R>(spy: Mock<A, R>, options?: WhenOptions): When<A, R>;
    /**
     * Replaces a module for everything that imports it afterwards. Called at
     * the top of a test file, it runs before that file's own imports. The
     * factory's object is the module's exports; `importOriginal()` loads the
     * real module. Mocks last the rest of the file. Not in browser runs.
     * An async factory's call returns a promise to await.
     */
    module(
      specifier: string,
      factory: (importOriginal: <M = Record<string, unknown>>() => Promise<M>) => Promise<object>,
    ): Promise<void>;
    module(
      specifier: string,
      factory: (importOriginal: <M = Record<string, unknown>>() => Promise<M>) => object,
    ): void;
    /** The real module, whether or not it is mocked. */
    importActual<M = Record<string, unknown>>(specifier: string): Promise<M>;
    /** Forgets every mock's calls. */
    clearAll(): typeof mock;
    /** …and how each was told to answer. */
    resetAll(): typeof mock;
    /**
     * Puts it all back: every spy's method, every replaced global and every
     * environment variable.
     */
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
