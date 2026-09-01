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

import { clock, describe, expect, it, mock, suite, test } from "runtime:test";

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
  expect({ a: 1 }).toMatchObject({ a: 1 });
  expect(1).not.toBe(2);

  // @ts-expect-error — `toBe` takes the type it was given.
  expect(2).toBe("two");
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

// --- clock ------------------------------------------------------------------

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
