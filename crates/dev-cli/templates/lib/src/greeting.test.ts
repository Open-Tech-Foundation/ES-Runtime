import { assertEquals, test } from "runtime:test";
import { greet } from "./greeting.ts";

test("it greets by name", () => {
  assertEquals(greet("world"), "Hello, world!");
});
