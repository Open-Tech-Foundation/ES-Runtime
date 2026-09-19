import { assertEquals } from "jsr:@std/assert@1.0.16";
import { cases } from "./cases.js";

Deno.test("the baseline has 24 uniquely named cases", () => {
  assertEquals(cases.length, 24);
  assertEquals(new Set(cases.map((test) => test.name)).size, cases.length);
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
