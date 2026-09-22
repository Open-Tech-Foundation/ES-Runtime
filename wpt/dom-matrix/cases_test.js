import { assertEquals } from "jsr:@std/assert@1.0.16";
import { cases } from "./cases.js";
import { classify, drifted } from "./report.js";

Deno.test("every case is uniquely named", () => {
  assertEquals(new Set(cases.map((test) => test.name)).size, cases.length);
});

Deno.test("the baseline covers the prioritized layout-free groups", () => {
  // A set, not a list: which group happens to be declared first is not a fact
  // about the coverage.
  assertEquals(
    new Set(cases.map((test) => test.group)),
    new Set(["tree", "events", "parsing", "selectors", "forms"]),
  );
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

Deno.test("a case drifts when any recorded column changes", () => {
  const recorded = { chrome: { result: 1 }, esdev: { result: 1 } };
  assertEquals(drifted(recorded, { chrome: { result: 1 }, esdev: { result: 1 } }), false);
  assertEquals(drifted(recorded, { chrome: { result: 1 }, esdev: { result: 2 } }), true);
  assertEquals(drifted(recorded, { chrome: { result: 1 }, esdev: { error: "TypeError" } }), true);
  assertEquals(drifted(undefined, { chrome: { result: 1 } }), true);
});
