import { expect, test } from "runtime:test";

// Runs under `esdev test --dom`; a plain `esdev test` has no document.
test.runIf(typeof document !== "undefined")("renders the greeting", async () => {
  document.body.innerHTML = '<main id="app"></main>';
  await import("./main.ts");
  expect(document.querySelector("#app h1")).toHaveTextContent("Hello, world!");
});
