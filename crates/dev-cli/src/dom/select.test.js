import { expect, test } from "runtime:test";
import { createTree } from "./tree.js";
import { createSelectors } from "./select.js";

const tree = createTree();
const { compareSpecificity, specificity } = createSelectors(tree);

test("counts ids, classes and types separately", () => {
  expect(specificity("*")).toEqual([0, 0, 0]);
  expect(specificity("li")).toEqual([0, 0, 1]);
  expect(specificity(".a")).toEqual([0, 1, 0]);
  expect(specificity("[href]")).toEqual([0, 1, 0]);
  expect(specificity("#a")).toEqual([1, 0, 0]);
  expect(specificity("ul li.a#b")).toEqual([1, 1, 2]);
  expect(specificity("ul > li + a ~ span")).toEqual([0, 0, 4]);
  expect(specificity("a:hover")).toEqual([0, 1, 1]);
  expect(specificity("li:nth-child(2)")).toEqual([0, 1, 1]);
});

test("takes the most specific argument of a logical pseudo-class", () => {
  expect(specificity(":is(#a, .b, c)")).toEqual([1, 0, 0]);
  expect(specificity(":not(.b, c)")).toEqual([0, 1, 0]);
  expect(specificity(":has(> #a)")).toEqual([1, 0, 0]);
  expect(specificity("div:where(#a, .b)")).toEqual([0, 0, 1]);
  expect(specificity("li:nth-child(2n of .a)")).toEqual([0, 2, 1]);
});

test("a selector list cascades at its most specific selector", () => {
  expect(specificity("#a, .b")).toEqual([1, 0, 0]);
  expect(specificity(".b, #a")).toEqual([1, 0, 0]);
});

test("compares specificity from left to right", () => {
  expect(compareSpecificity([1, 0, 0], [0, 9, 9]) > 0).toBe(true);
  expect(compareSpecificity([0, 1, 0], [0, 1, 9]) < 0).toBe(true);
  expect(compareSpecificity([0, 1, 1], [0, 1, 1])).toBe(0);
});
