import { test, assertEquals, assertThrows } from "runtime:test";
import { HttpError } from "./http.ts";
import { validateTitle } from "./tasks.ts";

test("a title is trimmed and returned", () => {
  assertEquals(validateTitle({ title: "  Read the README  " }), "Read the README");
});

test("a body that is not an object is refused", () => {
  // The three shapes JSON.parse happily produces that are not a record.
  for (const body of ["a string", 42, null, [1, 2], true]) {
    // Second argument is what the error must be, third is what to say when it
    // is not: a bare `assertThrows` would also pass on a TypeError from a bug.
    assertThrows(() => validateTitle(body), HttpError, `accepted ${JSON.stringify(body)}`);
  }
});

test("a missing or non-string title is refused", () => {
  assertThrows(() => validateTitle({}));
  assertThrows(() => validateTitle({ title: 42 }));
  assertThrows(() => validateTitle({ title: null }));
});

test("an empty or whitespace-only title is refused", () => {
  assertThrows(() => validateTitle({ title: "" }));
  assertThrows(() => validateTitle({ title: "   \n\t " }));
});

test("a title is bounded", () => {
  // An unbounded field from an unauthenticated client is how a store fills up.
  assertEquals(validateTitle({ title: "a".repeat(200) }).length, 200);
  assertThrows(() => validateTitle({ title: "a".repeat(201) }));
});

test("a prototype-polluting key is just a key", () => {
  // `JSON.parse` never sets a prototype, so this is data — but a template that
  // did not check would be the place somebody assumed otherwise.
  assertEquals(validateTitle({ title: "ok", __proto__: { admin: true } }), "ok");
  assertEquals(({} as { admin?: boolean }).admin, undefined);
});
