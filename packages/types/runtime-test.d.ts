declare module "runtime:test" {
  /**
   * What a test's body, and its `beforeEach` and `afterEach`, are handed —
   * with the fixtures a {@link TestAPI.extend | test.extend} test names.
   */
  export interface TestContext {
    readonly task: { readonly name: string };
    readonly expect: typeof expect;
    /** Stops the test here and reports it skipped. */
    skip(note?: string): never;
    /** …only when `condition` holds. */
    skip(condition: boolean, note?: string): void;
    readonly onTestFinished: typeof onTestFinished;
    readonly onTestFailed: typeof onTestFailed;
    /**
     * Registers a benchmark, in a `*.bench.*` file run by `esdev bench`.
     * Elsewhere, reading it throws.
     */
    readonly bench: Bench;
  }

  /** How long a benchmark is measured for. */
  export interface BenchOptions {
    /** Milliseconds of samples, at least. 500 by default. */
    time?: number;
    /** Samples, at least. 10 by default. */
    iterations?: number;
    /** Milliseconds of warm-up before measuring. 100 by default. */
    warmupTime?: number;
    /** Warm-up samples, at least. 5 by default. */
    warmupIterations?: number;
  }

  /** A benchmark's own options: when it is measured, and around each sample. */
  export interface BenchTaskOptions extends BenchOptions {
    /** Before each sample, outside the timing. */
    beforeEach?(): unknown;
    /** After each sample, outside the timing. */
    afterEach?(): unknown;
    /**
     * Writes the result to this file, relative to the project root, each time
     * the benchmark is measured; `bench.from` reads it back.
     */
    writeResult?: string;
  }

  /**
   * What a benchmark's function is called with. A function that calls
   * `start()` and `end()` is timed between them, on every call; one that calls
   * neither is timed whole.
   */
  export interface BenchTimer {
    start(): void;
    end(): void;
  }

  /** A statistic's spread, in its own unit. */
  export interface BenchStatistics {
    mean: number;
    min: number;
    max: number;
    p50: number;
    /** The 95% margin of the mean, as a percentage of it. */
    rme: number;
  }

  /** A result as `bench.from` accepts one from a function. */
  export type BenchResultData = {
    samples: number;
    latency: Partial<BenchResult["latency"]> & { mean: number; rme: number };
    throughput: Partial<BenchStatistics> & { mean: number };
    warnings?: readonly string[];
  };

  /** What measuring a benchmark found. */
  export interface BenchResult {
    readonly name: string;
    /** Samples taken. */
    readonly samples: number;
    /** Milliseconds a call took. */
    readonly latency: BenchStatistics & { p75: number; p99: number; p999: number; sd: number };
    /** Calls a second. */
    readonly throughput: BenchStatistics;
    /** What makes the result untrustworthy, if anything: a wide margin, work the engine may have removed, a section too short to time. */
    readonly warnings: readonly string[];
    /** Read by `bench.from`, not measured in this run. */
    readonly stored?: true;
  }

  /** A registered benchmark. */
  export interface BenchTask {
    readonly name: string;
    /** Measures it, prints its row, and resolves to its result. */
    run(options?: BenchOptions): Promise<BenchResult>;
  }

  export interface Bench {
    (name: string, fn: (b: BenchTimer) => unknown): BenchTask;
    (name: string, options: BenchTaskOptions, fn: (b: BenchTimer) => unknown): BenchTask;
    /**
     * A benchmark that is not measured: its result is read from a file
     * `writeResult` wrote, or returned by `source`.
     */
    from(name: string, source: string | (() => BenchResultData | Promise<BenchResultData>)): BenchTask;
    /**
     * Measures benchmarks side by side, a sample of each in turn, prints them
     * as a table, and resolves to their results by name.
     */
    compare(...tasks: [...BenchTask[], BenchOptions] | BenchTask[]): Promise<Map<string, BenchResult>>;
  }

  /** A test's body. */
  export type TestBody<Fixtures = {}> = (context: TestContext & Fixtures) => void | Promise<void>;

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
        /**
         * Its tags, for `--tags-filter` and for the options each tag in
         * `test.tags` gives it. Its own options win over a tag's.
         */
        tags?: TagName | TagName[];
        /**
         * Run alongside the concurrent tests next to it, or — `false` — not,
         * whatever its group says.
         */
        concurrent?: boolean;
      };

  /**
   * Which tags exist, for TypeScript: declare them once and a misspelt one is
   * a type error, as it is a run error with `strictTags`.
   *
   * ```ts
   * declare module "runtime:test" {
   *   interface TestTags {
   *     tags: "frontend" | "db" | "flaky";
   *   }
   * }
   * ```
   */
  export interface TestTags {}

  /** A tag name: one of {@link TestTags}' when declared, any string otherwise. */
  export type TagName = TestTags extends { tags: infer T extends string } ? T : string;

  /**
   * Whether a test with these tags is one the run's `--tags-filter` selects;
   * `true` when there is no filter. For set-up that only some tags need.
   */
  export function matchesTags(tags: TagName[]): boolean;

  /** The ways a test is registered: name, body, and optional options. */
  export type Register<Fixtures = {}> = {
    (name: string, fn: TestBody<Fixtures>, options?: TestOptions): void;
    (name: string, options: TestOptions, fn: TestBody<Fixtures>): void;
  };

  /** How long a fixture lives, and whether every test gets it. */
  export interface FixtureOptions {
    /**
     * `"test"` (the default) sets it up for each test that names it;
     * `"file"` once for the file. `"worker"` is `"file"` here, where each
     * file is its own process.
     */
    scope?: "test" | "file" | "worker";
    /** Set up for every test, named or not. */
    auto?: boolean;
  }

  /** What a builder fixture's function is handed beside the context. */
  export interface FixtureHelpers {
    /** Runs when the fixture's scope ends. Once per fixture. */
    onCleanup(fn: () => void | Promise<void>): void;
  }

  /** A builder fixture: its value, or a function returning it. */
  export type BuilderFixture<Fixtures, V> =
    | ((context: TestContext & Fixtures, helpers: FixtureHelpers) => V)
    | V;

  /** An object-syntax fixture: a value, or a function that hands it to `use`. */
  export type UseFixture<Context, V> =
    | V
    | ((context: Context, use: (value: V) => Promise<void>) => void | Promise<void>);

  /** `test`, and what `test.extend` returns: the same API, with fixtures. */
  export type TestAPI<Fixtures = {}> = Register<Fixtures> & {
    /**
     * Registers the case and reports it as **skipped** without running it —
     * counted in the tally rather than left out of it. The body may be left
     * out.
     */
    skip: Register<Fixtures> & ((name: string) => void) & { concurrent: Register<Fixtures> };
    /** Runs this case and skips the rest — the one you are working on. */
    only: Register<Fixtures> & { concurrent: Register<Fixtures> };
    /** A test known to fail: it passes while it fails, and fails once it passes. */
    fails: Register<Fixtures> & {
      each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
    };
    /** A name with no body yet, reported as skipped. */
    todo(name: string, fn?: () => void | Promise<void>): void;
    /**
     * Runs alongside the concurrent tests next to it — at most
     * `maxConcurrency` (5) at once. Hooks, fixtures, `expect.assertions`
     * and snapshots each still belong to their own test, through its awaits.
     */
    concurrent: Register<Fixtures> & {
      skip: Register<Fixtures> & ((name: string) => void);
      only: Register<Fixtures>;
      todo(name: string, fn?: () => void | Promise<void>): void;
      each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
    };
    /** Runs on its own, whatever its group says. */
    sequential: Register<Fixtures>;
    /** Registers the case only when the condition is false, and skips it otherwise. */
    skipIf(condition: unknown): TestFn<Fixtures>;
    /** The mirror: registers it only when the condition holds. */
    runIf(condition: unknown): TestFn<Fixtures>;
    /**
     * One case per row, named by substituting the row into the name —
     * `%s`/`%d`/`%i`/`%f`/`%j`/`%o`, `%#` for the index, `$key` for a property.
     * An array row is spread into the body's parameters.
     */
    each: Each<(name: string, fn: (...row: never[]) => void | Promise<void>) => void>;
    /**
     * A test with a fixture: a value its tests name in their first parameter,
     * set up for them and torn down after. The function may return a promise,
     * and registers its teardown with `onCleanup`. Chain it for more; a later
     * fixture reaches the earlier ones through its own first parameter.
     *
     * ```ts
     * const dbTest = test
     *   .extend("db", { scope: "file" }, async ({}, { onCleanup }) => {
     *     const db = await open();
     *     onCleanup(() => db.close());
     *     return db;
     *   })
     *   .extend("user", ({ db }) => db.createUser());
     *
     * dbTest("reads its user", ({ user }) => expect(user.id).toBeDefined());
     * ```
     */
    extend<K extends string, V>(
      name: K,
      fixture: BuilderFixture<Fixtures, V>,
    ): TestAPI<Fixtures & { [P in K]: Awaited<V> }>;
    extend<K extends string, V>(
      name: K,
      options: FixtureOptions,
      fixture: BuilderFixture<Fixtures, V>,
    ): TestAPI<Fixtures & { [P in K]: Awaited<V> }>;
    /**
     * Playwright's object syntax: each fixture a value, a function that hands
     * its value to `use` and tears down after `use` returns, or
     * `[fixture, options]`. Name the types, which `use` cannot infer:
     * `test.extend<{ page: Page }>({ page: async ({}, use) => … })`.
     */
    extend<T extends object>(fixtures: {
      [P in keyof T]:
        | UseFixture<TestContext & Fixtures & T, T[P]>
        | [UseFixture<TestContext & Fixtures & T, T[P]>, FixtureOptions];
    }): TestAPI<Fixtures & T>;
  };

  /**
   * Registers a test. It runs when the ones before it have finished.
   *
   * Cases run **one at a time**, in the order the file wrote them, unless
   * marked concurrent (`test.concurrent`). A test that awaits holds up the
   * next, deliberately: two tests sharing a database, a
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
  export const test: TestAPI;

  /** What `test` is, for the conditional forms that hand it back. */
  export type TestFn<Fixtures = {}> = Register<Fixtures> & {
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
  /** What a group may say about itself. */
  export interface DescribeOptions {
    /** Tags every test in the group carries. */
    tags?: TagName | TagName[];
    /** Whether the group's tests run concurrently, unless one says otherwise. */
    concurrent?: boolean;
  }

  export const describe: {
    (name: string, body: () => void): void;
    (name: string, options: DescribeOptions, body: () => void): void;
    /** Skips every test in the group, and reports each as skipped. */
    skip(name: string, body: () => void): void;
    skip(name: string, options: DescribeOptions, body: () => void): void;
    /** Runs this group and skips everything outside it. */
    only(name: string, body: () => void): void;
    only(name: string, options: DescribeOptions, body: () => void): void;
    /** A group planned and not written. Its name is reported as skipped. */
    todo(name: string, body?: () => void): void;
    /** Every test in the group runs concurrently, unless one says otherwise. */
    concurrent: {
      (name: string, body: () => void): void;
      (name: string, options: DescribeOptions, body: () => void): void;
      skip(name: string, body: () => void): void;
      only(name: string, body: () => void): void;
    };
    /** No test in the group runs concurrently, whatever encloses it. */
    sequential: {
      (name: string, body: () => void): void;
      (name: string, options: DescribeOptions, body: () => void): void;
    };
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

  /** `beforeEach` and `afterEach` are handed the test's context. */
  export type EachHook = (context: TestContext) => void | Promise<void>;

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
  export function beforeEach(fn: EachHook): void;

  /**
   * Runs after every test in scope, innermost group first, including one that
   * failed — it is cleanup, so it runs whatever happened. One that throws fails
   * the test unless the test had already failed.
   */
  export function afterEach(fn: EachHook): void;

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
    /**
     * {@link Matchers.toEqual}, and also: a key set to `undefined` is not a key
     * left out, a hole in an array is not an `undefined`, and a class instance
     * is not a plain object with the same fields.
     */
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
    /** `null` or `undefined`. */
    toBeNullable(): void;
    /**
     * A benchmark result's throughput is higher than `other`'s, by at least
     * `delta` (0.1 is 10%).
     */
    toBeFasterThan(other: BenchResult, options?: { delta?: number }): void;
    /** …lower than `other`'s, by at least `delta`. */
    toBeSlowerThan(other: BenchResult, options?: { delta?: number }): void;
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
    /** Called once, and with these arguments. */
    toHaveBeenCalledExactlyOnceWith(...args: unknown[]): void;
    /**
     * First called before `other` was first called. A mock never called fails,
     * unless `requireCall` is `false`.
     */
    toHaveBeenCalledBefore(other: Mock, requireCall?: boolean): void;
    /** First called after `other` was first called. */
    toHaveBeenCalledAfter(other: Mock, requireCall?: boolean): void;
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
    /**
     * Resolved at least once. A returned promise counts once it settles, so
     * await the call first; a value that is not a promise counts at once.
     */
    toHaveResolved(): void;
    toHaveResolvedTimes(n: number): void;
    toHaveResolvedWith(value: unknown): void;
    toHaveLastResolvedWith(value: unknown): void;
    toHaveNthResolvedWith(n: number, value: unknown): void;

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
    /** An array every element of which equals `item`, a value or a matcher. */
    arrayOf(item: unknown): any;
    /** A value the schema accepts. Needs a schema that validates synchronously. */
    schemaMatching(schema: StandardSchemaV1): any;
    /** The asymmetric matchers, inverted. */
    not: {
      stringContaining(part: string): any;
      stringMatching(pattern: string | RegExp): any;
      arrayContaining(wanted: unknown[]): any;
      objectContaining(wanted: object): any;
      arrayOf(item: unknown): any;
      schemaMatching(schema: StandardSchemaV1): any;
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
    /** Fails the test here. */
    fail(message?: string): never;
    /**
     * Decides equality for the pairs a tester recognises, in `toEqual` and
     * every other deep comparison, for the rest of the file. A tester returns
     * `true` or `false`, or `undefined` to pass the pair on.
     */
    addEqualityTesters(testers: EqualityTester[]): void;
    /** Prints the values `test` accepts in every snapshot for the rest of the file. */
    addSnapshotSerializer(serializer: SnapshotSerializer): void;
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

  /**
   * What global setup provides, by key. Declare each key here to provide and
   * inject it:
   *
   * ```ts
   * declare module "runtime:test" {
   *   interface ProvidedContext { dbUrl: string }
   * }
   * ```
   */
  export interface ProvidedContext {}

  /** The argument a global setup's `setup` function receives. */
  export interface GlobalSetupContext {
    /** Hands `value`, as JSON, to every test file, which reads it with {@link inject}. */
    provide<K extends keyof ProvidedContext & string>(key: K, value: ProvidedContext[K]): void;
  }

  /**
   * A value global setup provided, or `undefined`. Global setup is a module
   * named by `--global-setup` or `test.globalSetup` in esdev.json, exporting
   * `setup(context)` and `teardown()`, or a default function that returns its
   * teardown.
   */
  export function inject<K extends keyof ProvidedContext & string>(key: K): ProvidedContext[K];

  // --- type assertions -------------------------------------------------------

  /** @internal */ type IsAny<T> = 0 extends 1 & T ? true : false;
  /** @internal */ type IsNever<T> = [T] extends [never] ? true : false;
  /** @internal */ type IsUnknown<T> = IsAny<T> extends true ? false : unknown extends T ? true : false;
  /** @internal Identity, as TypeScript decides it: `any` is not `unknown`, and `readonly` counts. */
  type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends <T>() => T extends B ? 1 : 2 ? true : false;
  /** @internal */ type Extends<A, B> = IsNever<A> extends true ? IsNever<B> : [A] extends [B] ? true : false;
  /** @internal Whether a check came out as the chain wants: as it is, or under `.not`. */
  type Holds<Check extends boolean, Positive extends boolean> = Check extends Positive ? true : false;
  /** @internal Intersections and representation flattened, for `.branded`. */
  type DeepBrand<T> = T extends (...args: never[]) => unknown
    ? T
    : T extends object
      ? { [K in keyof T]: DeepBrand<T[K]> }
      : T;
  /**
   * @internal `A` narrowed to `E`'s keys, nested plain objects likewise, keeping
   * `A`'s own `readonly` and optional — so comparing it with `E` checks exactly
   * the properties `E` names.
   */
  type PickLike<A, E> = {
    [K in keyof A as K extends keyof E ? K : never]: A[K] extends (...args: never[]) => unknown
      ? A[K]
      : A[K] extends readonly unknown[]
        ? A[K]
        : A[K] extends object
          ? K extends keyof E
            ? E[K] extends object
              ? PickLike<A[K], E[K]>
              : A[K]
            : A[K]
          : A[K];
  };
  /** @internal */ type MatchesObject<A, E> = [A] extends [object]
    ? keyof E extends keyof A
      ? Equal<PickLike<A, E>, DeepBrand<E>> extends true
        ? true
        : Equal<DeepBrand<PickLike<A, E>>, DeepBrand<E>>
      : false
    : false;

  /**
   * Why a type assertion failed: what it expected, and what there was. It
   * shows up as the constraint the expected type does not satisfy.
   */
  export interface TypeMismatch<Expected, Actual> {
    readonly "✗ expected": Expected;
    readonly "✗ actual": Actual;
  }

  /**
   * Why a `toBe…` assertion failed. It has no call signature, so the call is
   * the error, and this is the type it names.
   */
  export interface TypeCheckFailed<Wanted extends string, Actual> {
    readonly "✗ wanted": Wanted;
    readonly "✗ actual": Actual;
  }

  /** @internal A `toBe…` assertion: callable when it holds. */
  type Is<Check extends boolean, Positive extends boolean, Wanted extends string, Actual> =
    Holds<Check, Positive> extends true
      ? () => true
      : TypeCheckFailed<Positive extends true ? Wanted : `not ${Wanted}`, Actual>;

  /**
   * Assertions about a type, checked by TypeScript and nothing at run time —
   * so `esdev check`, or `esdev test --typecheck`, is where they fail.
   */
  export interface ExpectTypeOf<Actual, Positive extends boolean = true> {
    /** The same assertions, each the other way round. */
    not: ExpectTypeOf<Actual, Positive extends true ? false : true>;
    /** Exactly this type: the same properties, `readonly` and optional included. */
    toEqualTypeOf<Expected>(
      this: Holds<Equal<Actual, Expected>, Positive> extends true ? unknown : TypeMismatch<Expected, Actual>,
      ...expected: [] | [Expected]
    ): true;
    /** Assignable to this type. */
    toExtend<Expected>(
      this: Holds<Extends<Actual, Expected>, Positive> extends true ? unknown : TypeMismatch<Expected, Actual>,
      ...expected: [] | [Expected]
    ): true;
    /** @deprecated {@link ExpectTypeOf.toExtend}, as expect-type renamed it. */
    toMatchTypeOf<Expected>(
      this: Holds<Extends<Actual, Expected>, Positive> extends true ? unknown : TypeMismatch<Expected, Actual>,
      ...expected: [] | [Expected]
    ): true;
    /**
     * An object type with at least these properties, each exactly as given —
     * stricter than {@link ExpectTypeOf.toExtend} about `readonly` and
     * optional, and checking nested objects the same way.
     */
    toMatchObjectType<Expected extends object>(
      this: Holds<MatchesObject<Actual, Expected>, Positive> extends true ? unknown : TypeMismatch<Expected, Actual>,
      ...expected: [] | [Expected]
    ): true;
    /** The members of a union that are assignable to `V`. */
    extract<V>(): ExpectTypeOf<Extract<Actual, V>, Positive>;
    /** The members of a union that are not. */
    exclude<V>(): ExpectTypeOf<Exclude<Actual, V>, Positive>;
    /** A function's return type. */
    returns: Actual extends (...args: never[]) => infer R ? ExpectTypeOf<R, Positive> : never;
    /** A function's parameters, as a tuple. */
    parameters: Actual extends (...args: infer P) => unknown ? ExpectTypeOf<P, Positive> : never;
    /** One parameter's type. */
    parameter<N extends number>(
      index: N,
    ): Actual extends (...args: infer P) => unknown ? ExpectTypeOf<P[N], Positive> : never;
    /** A class's constructor parameters, as a tuple. */
    constructorParameters: Actual extends abstract new (...args: infer P) => unknown
      ? ExpectTypeOf<P, Positive>
      : never;
    /** What `new` makes. */
    instance: Actual extends abstract new (...args: never[]) => infer I ? ExpectTypeOf<I, Positive> : never;
    /** An array's element type. */
    items: Actual extends readonly (infer I)[] ? ExpectTypeOf<I, Positive> : never;
    /** What a promise resolves to. */
    resolves: Actual extends PromiseLike<infer R> ? ExpectTypeOf<R, Positive> : never;
    /** What a type guard (`v is T`) narrows to. */
    // `any` where it would be `never`: a predicate's type must fit its parameter.
    guards: Actual extends (value: any, ...rest: any[]) => value is infer G ? ExpectTypeOf<G, Positive> : never;
    /** What an assertion function (`asserts v is T`) narrows to. */
    asserts: Actual extends (value: any, ...rest: any[]) => asserts value is infer A
      ? ExpectTypeOf<A, Positive>
      : never;
    /** Callable with these arguments. */
    toBeCallableWith: Actual extends (...args: infer P) => unknown ? (...args: P) => true : never;
    /** Constructible with these arguments. */
    toBeConstructibleWith: Actual extends abstract new (...args: infer P) => unknown
      ? (...args: P) => true
      : never;
    /** Has this property; the chain continues with its type. */
    toHaveProperty<K extends Positive extends true ? keyof Actual : PropertyKey>(
      this: Positive extends true
        ? unknown
        : K extends keyof Actual
          ? TypeMismatch<"no such property", K>
          : unknown,
      key: K,
    ): K extends keyof Actual ? ExpectTypeOf<Actual[K], Positive> : true;
    /** Equality that looks past how a type is written — `{ a: 1 } & { b: 1 }` is `{ a: 1; b: 1 }`. */
    branded: {
      toEqualTypeOf<Expected>(
        this: Holds<Equal<DeepBrand<Actual>, DeepBrand<Expected>>, Positive> extends true
          ? unknown
          : TypeMismatch<Expected, Actual>,
        ...expected: [] | [Expected]
      ): true;
    };
    toBeAny: Is<IsAny<Actual>, Positive, "any", Actual>;
    toBeUnknown: Is<IsUnknown<Actual>, Positive, "unknown", Actual>;
    toBeNever: Is<IsNever<Actual>, Positive, "never", Actual>;
    toBeFunction: Is<Extends<Actual, (...args: never[]) => unknown>, Positive, "a function", Actual>;
    toBeObject: Is<Extends<Actual, object>, Positive, "an object", Actual>;
    toBeArray: Is<Extends<Actual, readonly unknown[]>, Positive, "an array", Actual>;
    toBeString: Is<Extends<Actual, string>, Positive, "a string", Actual>;
    toBeNumber: Is<Extends<Actual, number>, Positive, "a number", Actual>;
    toBeBigInt: Is<Extends<Actual, bigint>, Positive, "a bigint", Actual>;
    toBeBoolean: Is<Extends<Actual, boolean>, Positive, "a boolean", Actual>;
    toBeSymbol: Is<Extends<Actual, symbol>, Positive, "a symbol", Actual>;
    toBeVoid: Is<Extends<Actual, void>, Positive, "void", Actual>;
    toBeNull: Is<Extends<Actual, null>, Positive, "null", Actual>;
    toBeUndefined: Is<Extends<Actual, undefined>, Positive, "undefined", Actual>;
    toBeNullable: Is<
      Equal<Actual, NonNullable<Actual>> extends true ? false : true,
      Positive,
      "nullable",
      Actual
    >;
  }

  /**
   * Assertions about the type of `actual`, or of `Actual` given alone —
   * checked by TypeScript, and nothing at run time.
   *
   * ```ts
   * expectTypeOf(parse).parameter(0).toBeString();
   * expectTypeOf(parse).returns.toEqualTypeOf<Result>();
   * expectTypeOf<Config>().toHaveProperty("port").toBeNumber();
   * ```
   */
  export function expectTypeOf<Actual>(actual?: Actual): ExpectTypeOf<Actual>;

  /** Checks, as a call would, that `value` is a `T`. Nothing at run time. */
  export function assertType<T>(value: T): void;

  /** What a {@link Mock} remembers. */
  /**
   * The validation half of a [Standard Schema](https://standardschema.dev),
   * the interface Zod, Valibot, ArkType and others implement.
   */
  export interface StandardSchemaV1 {
    readonly "~standard": {
      readonly version: 1;
      readonly vendor: string;
      readonly validate: (value: unknown) => StandardSchemaResult | Promise<StandardSchemaResult>;
    };
  }

  /** A validation's outcome: `issues` when it failed. */
  export type StandardSchemaResult =
    | { readonly value: unknown; readonly issues?: undefined }
    | { readonly issues: ReadonlyArray<unknown> };

  /** `this` inside an {@link EqualityTester}. */
  export interface TesterContext {
    /** Deep equality, testers included — for a tester comparing what it holds. */
    equals(a: unknown, b: unknown, testers?: EqualityTester[]): boolean;
  }

  /** Whether `a` equals `b`, or `undefined` for a pair it does not know. */
  export type EqualityTester = (
    this: TesterContext,
    a: unknown,
    b: unknown,
    testers: EqualityTester[],
  ) => boolean | undefined;

  /** The formatting options a {@link SnapshotSerializer} is given. */
  export interface SnapshotSerializerConfig {
    indent: string;
    [option: string]: unknown;
  }

  /** Prints a child value, at `indentation` when it starts a line of its own. */
  export type SnapshotPrinter = (
    value: unknown,
    config: SnapshotSerializerConfig,
    indentation: string,
    depth: number,
    refs: unknown[],
  ) => string;

  /** How a snapshot prints the values `test` accepts, in pretty-format's shape. */
  export type SnapshotSerializer =
    | {
        test(value: any): boolean;
        serialize(
          value: any,
          config: SnapshotSerializerConfig,
          indentation: string,
          depth: number,
          refs: unknown[],
          printer: SnapshotPrinter,
        ): string;
      }
    | {
        test(value: any): boolean;
        print(value: any, serialize: (value: unknown) => string, indent: (text: string) => string): string;
      };

  export interface MockRecord<A extends unknown[], R> {
    /** The arguments of every call, in order. */
    calls: A[];
    /** What each call did — returned a value, or threw one. */
    results: Array<{ type: "return"; value: R } | { type: "throw"; value: unknown }>;
    /**
     * What each call's returned promise came to: `"incomplete"` until it
     * settles. A value that is not a promise is `"fulfilled"` at once, and a
     * throw is `"rejected"`.
     */
    settledResults: Array<
      | { type: "fulfilled"; value: Awaited<R> }
      | { type: "rejected"; value: unknown }
      | { type: "incomplete"; value: undefined }
    >;
    /** `this` for each call — `undefined` for one made with `new`. */
    contexts: unknown[];
    /** Each call's place among the calls to every mock in the file, from 1. */
    invocationCallOrder: number[];
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
   * `freeze()` replaces the timers, `Date`, `performance.now`, `Temporal.Now`
   * and `Intl.DateTimeFormat`'s "now" — and, where the realm has them,
   * `setImmediate`, `requestAnimationFrame` and `requestIdleCallback` — so
   * everything scheduled through them moves only when the test says so. It is safe because a test file is a **process**: the swap
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
  /** What {@link clock.freeze} can replace. */
  export type Fakeable =
    | "setTimeout"
    | "clearTimeout"
    | "setInterval"
    | "clearInterval"
    | "setImmediate"
    | "clearImmediate"
    | "requestAnimationFrame"
    | "cancelAnimationFrame"
    | "requestIdleCallback"
    | "cancelIdleCallback"
    | "queueMicrotask"
    | "Date"
    | "performance"
    | "Temporal"
    | "Intl";

  interface FreezeBase {
    /** Where the clock starts. Default: now. */
    now?: Date | number | string;
    /** Timers `runAll` and `advance` fire before deciding the queue never drains. Default 10,000. */
    loopLimit?: number;
  }

  /**
   * Which parts of time `freeze` replaces. By default, all that the realm has
   * except `queueMicrotask`; `toFake` names the only ones, `toNotFake` the
   * ones to leave. Not both.
   */
  export type FreezeOptions =
    | (FreezeBase & { toFake?: Fakeable[]; toNotFake?: never })
    | (FreezeBase & { toNotFake?: Fakeable[]; toFake?: never });

  export const clock: {
    /** Stops time — at `at`, or wherever it is now. */
    freeze(at?: Date | number | string): typeof clock;
    /** …choosing what is replaced. */
    freeze(options: FreezeOptions): typeof clock;
    /** Starts it again, and puts the real ones back. Waiting timers are dropped. */
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
    /** To the next animation frame — they fall every 16ms — running what comes due. */
    advanceToNextFrame(): typeof clock;
    /** Jumps to whenever the next timer is due, and runs it. */
    next(): typeof clock;
    nextAsync(): Promise<typeof clock>;
    /** Runs the queue until it is empty, or refuses one that never drains. */
    runAll(): typeof clock;
    runAllAsync(): Promise<typeof clock>;
    /** Only what is waiting now — an interval fires once, not for ever. */
    runPending(): typeof clock;
    runPendingAsync(): Promise<typeof clock>;
    /** Runs the microtasks queued while `queueMicrotask` is faked. */
    runMicrotasks(): typeof clock;
    /** How many timers are waiting. */
    pending(): number;
    /** Drops them all without running any, and any faked microtasks. */
    clear(): typeof clock;
    /** Where the frozen clock stands. Timers do not fire because of it. */
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
