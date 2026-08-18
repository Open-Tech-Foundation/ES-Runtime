import { assert, assertEquals, test } from "runtime:test";
import { LEDE, LINKS, editHint } from "./page.ts";

test("the hint names the file it is pointing at", () => {
  assertEquals(editHint("src/main.ts"), "Edit src/main.ts and save.");
});

test("every link is absolute", () => {
  // They are rendered into a page that may be served from any path, so a
  // relative one would resolve against wherever the visitor happens to be.
  for (const link of LINKS) {
    assert(link.href.startsWith("https://"), link.href);
  }
});

test("the page says what it was built with", () => {
  assert(LEDE.includes("ES Runtime"), LEDE);
});
