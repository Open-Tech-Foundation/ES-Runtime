import { expect, test } from "runtime:test";
import { money, price } from "./catalog.js";

test("prices lines from the catalog, in cents", () => {
  const { lines, total } = price([
    { id: "mug", qty: 2 },
    { id: "pin", qty: 3 },
  ]);
  expect(lines.map((l) => l.subtotal)).toEqual([2800, 1500]);
  expect(total).toBe(4300);
});

test("drops lines it cannot price", () => {
  const { lines, total } = price([
    { id: "gone", qty: 1 },
    { id: "tee", qty: 0 },
  ]);
  expect(lines).toEqual([]);
  expect(total).toBe(0);
});

test("formats money", () => {
  expect(money(4300)).toBe("$43.00");
  expect(money(5)).toBe("$0.05");
});
