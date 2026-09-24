// A type test for `runtime:test`.
//
// **Why this exists.** These declarations describe a surface they do not
// implement, so nothing else can catch them being wrong: the runtime's own
// suite proves the *code* works, and would go on passing while the types
// beside it said something else entirely. That is the failure D71 made
// `runtime:test` a module to avoid — a `.ts` test file that runs perfectly and
// that `tsc --noEmit` rejects.
//
// **Both directions.** `@ts-expect-error` fails the build when the error it
// names *stops* happening, so a declaration that quietly widened to `any`
// breaks this file rather than passing it. The lines without it are the other
// half: legitimate usage must keep compiling.

import {
  assertSnapshot,
  assertType,
  beforeEach,
  type BenchResult,
  clock,
  describe,
  expect,
  expectTypeOf,
  type GlobalSetupContext,
  inject,
  it,
  matchesTags,
  mock,
  onTestFailed,
  onTestFinished,
  suite,
  test,
  waitFor,
} from "runtime:test";

// --- the vocabulary -----------------------------------------------------------

suite("aliases are the same functions", () => {
  it("registers like test", () => {
    expect(1).toBe(1);
  });
});

test.todo("planned");
test.skipIf(process_is_missing())("skipped when true", () => {});
test.runIf(true)("run when true", () => {});

declare function process_is_missing(): boolean;

test.each([
  [1, 1, 2],
  [2, 3, 5],
])("adds %d + %d = %d", (a: number, b: number, want: number) => {
  expect(a + b).toBe(want);
});

test.each([{ name: "ada" }, { name: "alan" }])("$name", (row) => {
  expect(row.name.length).toBeGreaterThan(0);
});

describe.each([["a"], ["b"]])("group %s", (letter: string) => {
  test("has a letter", () => expect(letter).toHaveLength(1));
});

// A row written `as const` is a tuple, and the body's parameters are checked
// against it. Without `as const` a row of one type infers as an array, so the
// body may take as many of that type as it likes — TypeScript's rule, not a
// looseness in these declarations, and worth pinning either way.
test.each([[1, 2]] as const)("a tuple row", (a, b) => {
  expect(a + b).toBe(3);
});
test.each([[1, 2]])("an array row", (a: number, b: number, c: number) => {
  expect(a + b).toBe(3);
  expect(c).toBeUndefined();
});

// --- expect -----------------------------------------------------------------

test("matchers are typed against the value", () => {
  expect(2).toBe(2);
  expect("a").toBe("a");
  expect([1, 2]).toEqual([1, 2]);
  expect({ answer: 42 }).toMatchSnapshot();
  expect({ answer: 42 }).toMatchSnapshot("answer");
  expect({ id: 42 }).toMatchSnapshot({ id: expect.any(Number) });
  expect("<main />").toMatchFileSnapshot("home.html");
  assertSnapshot({ answer: 42 }, "answer assertion");
  expect(() => {
    throw new Error("broken");
  }).toThrowErrorMatchingSnapshot();
  expect({ a: 1 }).toMatchObject({ a: 1 });
  expect(1).not.toBe(2);

  // @ts-expect-error — `toBe` takes the type it was given.
  expect(2).toBe("two");
  // @ts-expect-error — a snapshot name is a string when supplied.
  expect(2).toMatchSnapshot(2);
  // @ts-expect-error — there is no such matcher.
  expect(2).toBeAlmostCertainly(2);
  // @ts-expect-error — and `.not` carries the same set, not a wider one.
  expect(2).not.toBeAlmostCertainly(2);
});

test("the awaited forms are promises", async () => {
  await expect(Promise.resolve(1)).resolves.toBe(1);
  await expect(Promise.reject(new Error("x"))).rejects.toThrow("x");
  await expect(Promise.resolve(1)).resolves.not.toBe(2);

  // `.resolves` unwraps, so the matcher sees the resolved type.
  // @ts-expect-error
  await expect(Promise.resolve(1)).resolves.toBe("one");
});

test("asymmetric matchers go where a value goes", () => {
  expect({ id: 1, name: "ada" }).toEqual({
    id: expect.any(Number),
    name: expect.stringContaining("ad"),
  });
  expect([1]).toEqual(expect.arrayContaining([1]));
  expect("x").toEqual(expect.stringMatching(/x/));
  expect({ a: 1 }).toEqual(expect.objectContaining({ a: 1 }));
  expect(1).toEqual(expect.anything());
});

// --- mock -------------------------------------------------------------------

test("a mock keeps the signature it was made with", () => {
  const double = mock.fn((n: number) => n * 2);
  const doubled: number = double(2);
  const calls: number[][] = double.mock.calls;
  void doubled;
  void calls;

  double.mockReturnValue(4);
  double.mockImplementationOnce((n) => n + 1);
  double.mockClear().mockReset().mockName("double");

  // @ts-expect-error — the argument list is the one it was declared with.
  double("two");
  // @ts-expect-error — and so is the return type.
  const wrong: string = double(2);
  void wrong;
  // @ts-expect-error — `mockReturnValue` answers with the return type.
  double.mockReturnValue("four");
});

test("an untyped mock stays usable", () => {
  const anything = mock.fn();
  anything(1, "two", {});
  anything.mockResolvedValue(1);
  expect(anything).toHaveBeenCalledWith(1, "two", {});
});

test("spyOn needs an object and a key of it", () => {
  const client = { post: (path: string) => path.length };
  const spy = mock.spyOn(client, "post");
  spy.mockRestore();

  // @ts-expect-error — a number has no methods to take.
  mock.spyOn(42, "toFixed");
  // @ts-expect-error — and the key has to be one it has.
  mock.spyOn(client, "put");
});

test("mock.module takes exports, now or later", async () => {
  const sync: void = mock.module("./mail.ts", () => ({ send: mock.fn() }));
  const later: Promise<void> = mock.module("./mail.ts", async (importOriginal) => ({
    ...(await importOriginal()),
    send: mock.fn(),
  }));
  await later;
  const real = await mock.importActual<{ send(to: string): string }>("./mail.ts");
  real.send("ada");

  // @ts-expect-error — a module's exports are an object.
  mock.module("./mail.ts", () => 42);
  // @ts-expect-error — and there is a factory to give them.
  mock.module("./mail.ts");
  return sync;
});

test("mocks throw, answer for a while, and answer by argument", async () => {
  const load = mock.fn((id: number) => `user ${id}`);
  load.mockThrowOnce(new Error("down")).mockThrow("always");
  const impl: ((id: number) => string) | undefined = load.getMockImplementation();
  load.withImplementation(() => "temporary", () => load(1));
  const later: Promise<void> = load.withImplementation(() => "t", async () => {});
  await later;

  {
    using spy = mock.spyOn(console, "log");
    using answers = mock.when(load, { onUnmatched: "throw" })
      .calledWith(1)
      .thenReturn("one")
      .thenReturnOnce("first")
      .calledWith(expect.any(Number))
      .thenThrow(new Error("no"), { times: 2 });
    expect(answers).toHaveBeenExhausted();
    spy.mockClear();
    // @ts-expect-error — the answer is what the mock returns.
    answers.thenReturn(42);
    // @ts-expect-error — and the arguments are the mock's.
    answers.calledWith("one");
  }

  mock.env("PORT", "8080").env("DEBUG", undefined);
  // @ts-expect-error — an environment holds strings.
  mock.env("PORT", 8080);
  impl?.(1);
});

test("call order, resolved values and the new asymmetric matchers", async () => {
  const load = mock.fn(async (id: number) => ({ id }));
  const save = mock.fn();
  await load(1);
  save();
  expect(load).toHaveBeenCalledBefore(save);
  expect(save).toHaveBeenCalledAfter(load, false);
  expect(load).toHaveBeenCalledExactlyOnceWith(1);
  expect(load).toHaveResolvedWith({ id: 1 });
  expect(load).toHaveNthResolvedWith(1, { id: 1 });
  const order: number = load.mock.invocationCallOrder[0];
  const settled = load.mock.settledResults[0];
  if (settled.type === "fulfilled") settled.value.id satisfies number;
  expect(null).toBeNullable();
  expect(["a"]).toEqual(expect.arrayOf(expect.any(String)));
  expect([1]).toEqual(expect.not.arrayOf(expect.any(String)));
  const schema = { "~standard": { version: 1 as const, vendor: "x", validate: (v: unknown) => ({ value: v }) } };
  expect("a").toEqual(expect.schemaMatching(schema));
  // @ts-expect-error — a schema has a ~standard member.
  expect.schemaMatching({});
  // @ts-expect-error — the other has to be a mock.
  expect(load).toHaveBeenCalledBefore(() => {});
  order satisfies number;
});

test("equality testers and snapshot serializers", () => {
  expect.addEqualityTesters([
    function (a, b, testers) {
      if (Array.isArray(a) && Array.isArray(b)) return this.equals(a.length, b.length, testers);
      return undefined;
    },
  ]);
  expect.addSnapshotSerializer({
    test: (v) => v instanceof Date,
    serialize: (v, config, indentation, depth, refs, printer) =>
      `Date ${printer(v.toISOString(), config, indentation, depth, refs)}`,
  });
  expect.addSnapshotSerializer({ test: () => false, print: (v, serialize, indent) => indent(serialize(v)) });
  // @ts-expect-error — a tester is a function.
  expect.addEqualityTesters([1]);
  if (Math.random() > 2) expect.fail("never");
});

declare module "runtime:test" {
  interface ProvidedContext {
    port: number;
  }
}

test("inject reads what global setup provided", () => {
  const port: number = inject("port");
  // @ts-expect-error — only a declared key can be injected.
  inject("anything");
  const setup = (context: GlobalSetupContext) => {
    context.provide("port", 4321);
    // @ts-expect-error — a provided key has its declared type.
    context.provide("port", "4321");
  };
  setup({ provide() {} });
  port satisfies number;
});

// --- fixtures -----------------------------------------------------------------

const dbTest = test
  .extend("config", { port: 3000, host: "localhost" })
  .extend("db", { scope: "file" }, async ({ config }, { onCleanup }) => {
    const port: number = config.port;
    onCleanup(() => {});
    return { port, rows: [] as number[] };
  })
  .extend("user", ({ db }) => ({ id: db.rows.length, name: "ada" }));

dbTest("fixtures arrive typed", ({ user, db, config, task, skip, expect: check }) => {
  const id: number = user.id;
  const name: string = task.name;
  check(db.port).toBe(config.port);
  skip(id > 1);
  // @ts-expect-error — a fixture's type is what it returned.
  const wrong: string = user.id;
  return void [wrong, name];
});

// @ts-expect-error — only fixtures that were declared.
dbTest("unknown fixture", ({ missing }) => missing);

const pageTest = dbTest.extend<{ page: string }>({
  page: async ({ user }, use) => {
    await use(`page for ${user.name}`);
  },
});
pageTest("object syntax", ({ page }) => {
  const text: string = page;
  return void text;
});
pageTest.skip("variants keep the fixtures", ({ page }) => void page);

beforeEach(({ task }) => void task.name);

// --- tags ---------------------------------------------------------------------

declare module "runtime:test" {
  interface TestTags {
    tags: "db" | "flaky" | "unit/components";
  }
}

test("tagged", { tags: ["db", "flaky"], timeout: 1000 }, () => {});
test("one tag", { tags: "unit/components" }, () => {});
describe("a tagged group", { tags: "db" }, () => {});
describe.skip("a skipped tagged group", { tags: ["flaky"] }, () => {});
const needsDb: boolean = matchesTags(["db"]);
// @ts-expect-error — a tag that was not declared.
test("misspelt", { tags: "dbb" }, () => {});
// @ts-expect-error — nor in matchesTags.
matchesTags(["frontend"]);
void needsDb;

// --- expectTypeOf -------------------------------------------------------------

declare function parse(text: string, strict?: boolean): { ok: boolean };
declare function isString(v: unknown): v is string;
declare function assertNumber(v: unknown): asserts v is number;

test("type assertions", () => {
  class Box { constructor(public size: number) {} }

  expectTypeOf({ a: 1 }).toEqualTypeOf<{ a: number }>();
  expectTypeOf({ a: 1 }).toEqualTypeOf({ a: 2 });
  expectTypeOf({ a: 1, b: 1 }).not.toEqualTypeOf<{ a: number }>();
  expectTypeOf({ a: 1, b: 1 }).toExtend<{ a: number }>();
  expectTypeOf<number>().toExtend<string | number>();
  expectTypeOf<string | number>().not.toExtend<number>();
  expectTypeOf({ a: 1, b: 2 }).toMatchObjectType<{ a: number }>();
  expectTypeOf({ name: "J", address: { city: "NY", zip: "1" } }).toMatchObjectType<{ name: string; address: { city: string } }>();
  expectTypeOf(parse).parameter(0).toBeString();
  expectTypeOf(parse).parameters.toEqualTypeOf<[text: string, strict?: boolean]>();
  expectTypeOf(parse).returns.toEqualTypeOf<{ ok: boolean }>();
  expectTypeOf(parse).toBeCallableWith("x");
  expectTypeOf(parse).toBeCallableWith("x", true);
  expectTypeOf(Box).toBeConstructibleWith(3);
  expectTypeOf(Box).instance.toHaveProperty("size").toBeNumber();
  expectTypeOf(Box).constructorParameters.toEqualTypeOf<[size: number]>();
  expectTypeOf([1, 2]).items.toBeNumber();
  expectTypeOf(Promise.resolve("x")).resolves.toBeString();
  expectTypeOf(isString).guards.toBeString();
  expectTypeOf(assertNumber).asserts.toBeNumber();
  expectTypeOf<"a" | 1>().extract<string>().toEqualTypeOf<"a">();
  expectTypeOf<"a" | 1>().exclude<string>().toEqualTypeOf<1>();
  expectTypeOf({ a: 1 }).toHaveProperty("a");
  expectTypeOf({ a: 1 }).not.toHaveProperty("c");
  expectTypeOf<{ a: { b: 1 } & { c: 1 } }>().branded.toEqualTypeOf<{ a: { b: 1; c: 1 } }>();
  expectTypeOf<any>().toBeAny();
  expectTypeOf<unknown>().toBeUnknown();
  expectTypeOf<never>().toBeNever();
  expectTypeOf(1).not.toBeString();
  expectTypeOf<string | null>().toBeNullable();
  expectTypeOf<string>().not.toBeNullable();
  expectTypeOf(() => {}).toBeFunction();
  expectTypeOf(null).toBeNull();
  expectTypeOf(undefined).toBeUndefined();
  expectTypeOf(1n).toBeBigInt();
  expectTypeOf(Symbol()).toBeSymbol();
  expectTypeOf([1]).toBeArray();
  expectTypeOf({}).toBeObject();
  expectTypeOf(true).toBeBoolean();
  assertType<number>(1);

  // The failures, each an error.
  // @ts-expect-error — a property of the wrong type.
  expectTypeOf({ a: 1 }).toEqualTypeOf<{ a: string }>();
  // @ts-expect-error — any is not unknown.
  expectTypeOf<any>().toEqualTypeOf<unknown>();
  // @ts-expect-error — readonly counts.
  expectTypeOf<{ readonly a: number }>().toEqualTypeOf<{ a: number }>();
  // @ts-expect-error — not a string.
  expectTypeOf(1).toBeString();
  // @ts-expect-error — does extend.
  expectTypeOf<number>().not.toExtend<string | number>();
  // @ts-expect-error — readonly under toMatchObjectType.
  expectTypeOf<{ readonly a: number; b: 1 }>().toMatchObjectType<{ a: number }>();
  // @ts-expect-error — no such property.
  expectTypeOf({ a: 1 }).toHaveProperty("c");
  // @ts-expect-error — it has it.
  expectTypeOf({ a: 1 }).not.toHaveProperty("a");
  // @ts-expect-error — the wrong argument.
  expectTypeOf(parse).toBeCallableWith(1);
  // @ts-expect-error — representation differs without branded.
  expectTypeOf<{ a: { b: 1 } & { c: 1 } }>().toEqualTypeOf<{ a: { b: 1; c: 1 } }>();
  // @ts-expect-error — assertType checks its argument.
  assertType<string>(1);
});

// --- concurrency --------------------------------------------------------------

test.concurrent("runs alongside", async ({ expect: check }) => {
  check(1).toBe(1);
});
test.concurrent.skip("skipped", async () => {});
test.skip.concurrent("skipped too", async () => {});
test.concurrent.each([[1], [2]])("row %d", async (n) => void n);
test.sequential("alone", () => {});
test("an option", { concurrent: true }, async () => {});
describe.concurrent("a concurrent group", () => {});
describe("a group", { concurrent: false }, () => {});
describe.sequential("a sequential group", () => {});
dbTest.concurrent("fixtures and concurrency", async ({ user }) => void user.id);

// --- clock ------------------------------------------------------------------

test("the clock can be told what to fake", () => {
  clock.freeze({ now: 0, toFake: ["setTimeout", "Date", "queueMicrotask"], loopLimit: 50 });
  clock.freeze({ toNotFake: ["performance"] });
  clock.freeze({ now: new Date() });
  clock.advanceToNextFrame().runMicrotasks().release();

  // @ts-expect-error — nextTick is Node's, and there is none to fake.
  clock.freeze({ toFake: ["nextTick"] });
  // @ts-expect-error — one list or the other.
  clock.freeze({ toFake: ["Date"], toNotFake: ["Intl"] });
});

test("the clock takes milliseconds and moments", async () => {
  clock.freeze();
  clock.freeze(new Date());
  clock.freeze(0);
  clock.freeze("2020-01-01");
  clock.advance(100);
  await clock.advanceAsync(100);
  await clock.runAllAsync();
  const waiting: number = clock.pending();
  void waiting;
  clock.release();

  // @ts-expect-error — an advance is a duration, not a date.
  clock.advance(new Date());
  // @ts-expect-error — the async form is awaited, not chained synchronously.
  clock.advanceAsync(1).release();
});

// --- per-test options, test.fails, and cleanup ---------------------------------

test("a timeout after the body", () => {}, 5000);
test("options after the body", () => {}, { timeout: 100, retry: 2, repeats: 3 });
test("options before the body", { retry: 1 }, () => {});
test.only("only takes options too", () => {}, { timeout: 10 });
test.fails("a known failure", () => {
  expect(1).toBe(2);
});
// @ts-expect-error — a retry count is a number.
test("retry is a number", () => {}, { retry: "twice" });

test("cleanup belongs to the test", () => {
  onTestFinished(() => {});
  onTestFailed((error) => {
    void error;
  });
});

// --- expect's utilities --------------------------------------------------------

test("counting, soft, poll and waitFor", async () => {
  expect.assertions(2);
  expect.hasAssertions();
  expect.soft(1).toBe(1);
  expect.soft("a").not.toBe("b");
  await expect.poll(() => 3, { timeout: 100, interval: 5 }).toBeGreaterThan(2);
  await expect.poll(async () => "x").not.toBe("y");
  const value: number = await waitFor(() => 7, { timeout: 50 });
  void value;
  // @ts-expect-error — poll's matchers are awaited, and toBe keeps its type.
  await expect.poll(() => 3).toBe("three");
  if (value < 0) expect.unreachable("never negative");
  expect(4).toSatisfy((n) => n > 3);
  expect("b").toBeOneOf(["a", "b"]);
  expect({ x: 0.3 }).toEqual({ x: expect.closeTo(0.3), y: expect.not.stringContaining("z") });
});

// Custom matchers: added at runtime, declared by augmentation.
declare module "runtime:test" {
  interface Matchers<T> {
    toBeWithin(lo: number, hi: number): void;
  }
  interface AsymmetricMatchers {
    toBeWithin(lo: number, hi: number): any;
  }
}

expect.extend({
  toBeWithin(received: number, lo: number, hi: number) {
    return {
      pass: received >= lo && received <= hi,
      message: () => `${this.isNot ? "not " : ""}within`,
    };
  },
});

test("an extended matcher", async () => {
  expect(5).toBeWithin(1, 10);
  expect(50).not.toBeWithin(1, 10);
  expect({ n: 3 }).toEqual({ n: expect.toBeWithin(1, 5) });
  await expect(Promise.resolve(4)).resolves.toBeWithin(1, 5);
  // @ts-expect-error — an extend entry is a function.
  expect.extend({ notAFunction: 1 });
});

// --- DOM matchers ---------------------------------------------------------------

declare const element: HTMLElement;

test("DOM matchers", () => {
  expect(element).toBeInTheDocument();
  expect(null).not.toBeInTheDocument();
  expect(element).toBeVisible();
  expect(element).toHaveTextContent(/hello/, { normalizeWhitespace: false });
  expect(element).toHaveAttribute("id", "x");
  expect(element).toHaveClass("a", "b", { exact: true });
  expect(element).toHaveValue(["a"]);
  expect(element).toBeChecked();
  expect(element).toBeDisabled();
  expect(element).toBeEnabled();
  expect(element).toBeRequired();
  expect(element).toHaveFocus();
  expect(element).toBeEmptyDOMElement();
  expect(element).toContainElement(null);
  expect(element).toContainHTML("<b>x</b>");
  expect(element).toHaveStyle({ marginTop: "4px", opacity: 1 });
  expect(element).toHaveStyle("display: none");
  // @ts-expect-error — text is a string or a RegExp.
  expect(element).toHaveTextContent(42);
});

// --- inline snapshots ------------------------------------------------------------

test("inline snapshots", () => {
  expect(1).toMatchInlineSnapshot();
  expect(1).toMatchInlineSnapshot(`1`);
  expect({ id: 1 }).toMatchInlineSnapshot({ id: expect.any(Number) }, `{}`);
  expect(() => {
    throw new Error("x");
  }).toThrowErrorMatchingInlineSnapshot(`Error("x")`);
  // @ts-expect-error — the snapshot is a string.
  expect(1).toMatchInlineSnapshot(1);
});

// --- benchmarks ------------------------------------------------------------------

test("benchmarks", async ({ bench }) => {
  const one = bench("parse", () => JSON.parse("{}"));
  const two = bench("async", { beforeEach: () => {}, time: 100 }, async () => {});
  const alone: BenchResult = await one.run({ iterations: 20 });
  const results: Map<string, BenchResult> = await bench.compare(one, two, { time: 200 });
  await bench.compare(one, two);
  const mean: number = alone.latency.p99 + alone.throughput.mean;
  void mean;
  expect(results.get("parse")!).toBeFasterThan(alone, { delta: 0.1 });
  expect(alone).toBeSlowerThan(results.get("async")!);
  // @ts-expect-error — a benchmark needs a function to measure.
  bench("nothing");
  const section = bench("section", { writeResult: "./bench/section.json" }, (b) => {
    b.start();
    b.end();
  });
  const stored = await bench.compare(
    section,
    bench.from("previous", "./bench/section.json"),
    bench.from("inline", () => ({ samples: 1, latency: { mean: 1, rme: 0 }, throughput: { mean: 1000 } })),
  );
  const warned: readonly string[] = stored.get("previous")!.warnings;
  void warned;
  // @ts-expect-error — writeResult is a path.
  bench("x", { writeResult: true }, () => {});
});
