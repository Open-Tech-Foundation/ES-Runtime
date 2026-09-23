import { expect, test } from "runtime:test";
import { hello } from "./index.ts";

test("greets by name", () => {
  expect(hello("world")).toBe("Hello, world!");
});
