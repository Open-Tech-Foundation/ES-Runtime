import { expect, test } from "runtime:test";

// Runs under `esdev test --dom`; a plain `esdev test` has no document.
test.runIf(typeof document !== "undefined")("renders the greeting", async () => {
  const { HelloWorld } = await import("./hello-world.ts");
  document.body.innerHTML = "<hello-world></hello-world>";
  const el = document.querySelector("hello-world") as InstanceType<typeof HelloWorld>;
  await el.updateComplete;
  expect(el.shadowRoot?.querySelector("h1")).toHaveTextContent("Hello, world!");
});
