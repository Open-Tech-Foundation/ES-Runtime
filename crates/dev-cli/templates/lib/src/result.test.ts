import { attempt, err, ok } from "./result.ts";

test("ok and err narrow on the discriminant", () => {
  const good = ok(42);
  assert(good.ok);
  assertEquals(good.value, 42);

  const bad = err(new Error("no"));
  assert(!bad.ok);
  assertEquals(bad.error.message, "no");
});

test("attempt turns a throw into a result", () => {
  assertEquals(attempt(() => 1), { ok: true, value: 1 });

  const failed = attempt(() => {
    throw new Error("boom");
  });
  assert(!failed.ok);
  assertEquals(failed.error.message, "boom");
});

test("a thrown non-Error becomes one", () => {
  // `throw "a string"` is legal and common in code nobody meant to write. A
  // caller should still get something with a `.message`.
  const failed = attempt(() => {
    throw "a string";
  });
  assert(!failed.ok);
  assert(failed.error instanceof Error);
  assertEquals(failed.error.message, "a string");
});
