import { assertEquals } from "jsr:@std/assert@1.0.16";
import { cases } from "./cases.js";
import { classify } from "./report.js";

Deno.test("the baseline has 29 uniquely named cases", () => {
  assertEquals(cases.length, 29);
  assertEquals(new Set(cases.map((test) => test.name)).size, 29);
});

Deno.test("the baseline covers the prioritized layout-free groups", () => {
  assertEquals([...new Set(cases.map((test) => test.group))], [
    "tree",
    "events",
    "parsing",
    "selectors",
    "forms",
  ]);
});

Deno.test("strict parsing is declared as an intentional esdev limit", () => {
  assertEquals(
    cases.filter((test) => test.limit).map((
      test,
    ) => [test.name, test.expectedEsdev]),
    [["malformed-markup-is-a-strict-esdev-limit", { result: "SyntaxError" }]],
  );
});

Deno.test("Chrome determines matches even when an emulator differs", () => {
  assertEquals(
    classify({}, { chrome: { result: true }, esdev: { result: true } }),
    "match",
  );
  assertEquals(
    classify({}, { chrome: { result: true }, esdev: { result: false } }),
    "gap",
  );
});

Deno.test("documented esdev limits override a Chrome difference", () => {
  assertEquals(
    classify(
      { expectedEsdev: { result: "SyntaxError" }, limit: "strict parser" },
      { chrome: { result: null }, esdev: { result: "SyntaxError" } },
    ),
    "intentional-limit",
  );
});
