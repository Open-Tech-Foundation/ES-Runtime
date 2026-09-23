import { expect, test } from "runtime:test";
import { hello } from "./hello.ts";

test("greets by name", () => {
  expect(hello("world")).toBe("Hello, world!");
});
