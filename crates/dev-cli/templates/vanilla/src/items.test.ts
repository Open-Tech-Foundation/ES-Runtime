import { formatCount, nextId, type Item } from "./items.ts";

const items = (...ids: number[]): Item[] => ids.map((id) => ({ id, label: `Item ${id}` }));

test("an id is one past the largest, not the length", () => {
  assertEquals(nextId([]), 1);
  assertEquals(nextId(items(1, 2, 3)), 4);
});

test("an id is still unused after something in the middle is removed", () => {
  // The bug `length + 1` has: remove item 1 of [1,2] and it hands out 2, which
  // the survivor already holds — and then two rows are the same row.
  assertEquals(nextId(items(2)), 3);
  assertEquals(nextId(items(1, 5)), 6);
});

test("the count reads as a sentence", () => {
  assertEquals(formatCount(0), "Nothing here yet.");
  assertEquals(formatCount(1), "1 item.");
  assertEquals(formatCount(2), "2 items.");
});
