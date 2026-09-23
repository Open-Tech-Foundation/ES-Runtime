import { expect, test } from "runtime:test";
import { createTree } from "./tree.js";
import { createRanges } from "./range.js";

// `createContextualFragment` is the only thing ranges ask of the parser, and
// these cases are about boundary points.
const parse = { parseFragment: () => { throw new Error("not parsed here"); } };

function fixture() {
  const tree = createTree();
  const ranges = createRanges(tree, parse);
  const document = new tree.HTMLDocument();
  ranges.install();
  const root = document.createElement("main");
  document.appendChild(root);
  return { document, ranges, root, tree };
}

test("exposes readonly boundary points on both range interfaces", () => {
  const { document, ranges, root } = fixture();
  const text = document.createTextNode("hello");
  root.appendChild(text);
  const range = document.createRange();
  range.setStart(text, 1);
  range.setEnd(text, 3);

  expect(range).toBeInstanceOf(ranges.AbstractRange);
  expect([range.startOffset, range.endOffset, range.collapsed]).toEqual([1, 3, false]);
  expect(() => { range.startOffset = 9; }).toThrow();
  expect(range.startOffset).toBe(1);
});

test("a static range snapshots what a live range follows", () => {
  const { document, ranges, root } = fixture();
  const text = document.createTextNode("hello");
  root.appendChild(text);
  const live = document.createRange();
  live.setStart(text, 1);
  live.setEnd(text, 4);
  const snapshot = new ranges.StaticRange({ startContainer: text, startOffset: 1, endContainer: text, endOffset: 4 });

  expect(snapshot).toBeInstanceOf(ranges.AbstractRange);
  expect(snapshot).not.toBeInstanceOf(ranges.Range);
  // Setting `data` replaces all of it, which collapses a live range inside the
  // node to its start (Chrome agrees); the static one does not move.
  text.data = "hi";
  expect([live.endOffset, snapshot.endOffset]).toEqual([0, 4]);
});

test("refuses a static range that cannot have boundary points", () => {
  const { document, ranges } = fixture();
  const doctype = document.implementation.createDocumentType("html", "", "");

  expect(() => new ranges.StaticRange({ startContainer: doctype, startOffset: 0, endContainer: doctype, endOffset: 0 }))
    .toThrow("doctype");
  expect(() => new ranges.StaticRange({})).toThrow("Node");
  expect(() => new ranges.AbstractRange()).toThrow("Illegal constructor");
});
