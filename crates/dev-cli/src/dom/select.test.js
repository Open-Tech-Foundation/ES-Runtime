import { expect, test } from "runtime:test";
import { createTree } from "./tree.js";
import { createSelectors } from "./select.js";

const tree = createTree();
const selectors = createSelectors(tree);
const { compareSpecificity, specificity } = selectors;
selectors.install();

function fixture(html) {
  const document = new tree.Document();
  const root = document.createElement("main");
  document.appendChild(root);
  const add = (name, attributes) => {
    const element = document.createElement(name);
    for (const [key, value] of Object.entries(attributes)) element.setAttribute(key, value);
    root.appendChild(element);
    return element;
  };
  void html;
  return { add, document, root };
}

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

test("resolves escapes in an identifier", () => {
  const { add, root } = fixture();
  const dotted = add("a", { id: "id.with.dots" });
  const colon = add("b", { class: "foo:bar" });
  const leading = add("u", { id: "1leading" });
  const accented = add("s", { class: "café" });

  expect(root.querySelector("#id\\.with\\.dots")).toBe(dotted);
  expect(root.querySelector(".foo\\:bar")).toBe(colon);
  // A hex escape, with the space that terminates it rather than a combinator.
  expect(root.querySelector("#\\31 leading")).toBe(leading);
  expect(root.querySelector(".caf\\e9")).toBe(accented);
  expect(root.querySelector(".café")).toBe(accented);
  expect(dotted.matches("#id\\.with\\.dots")).toBe(true);
  expect(Array.from(root.querySelectorAll("#id\\.with\\.dots, .foo\\:bar"))).toEqual([dotted, colon]);
  expect(root.querySelector("#id\\.with\\.dots + .foo\\:bar")).toBe(colon);
  // A class cannot contain a space, so an escaped one matches nothing.
  expect(root.querySelector(".a\\ b")).toBeNull();
  expect(() => root.querySelector("#")).toThrow("expected a name");
  expect(specificity("#id\\.with\\.dots")).toEqual([1, 0, 0]);
});
