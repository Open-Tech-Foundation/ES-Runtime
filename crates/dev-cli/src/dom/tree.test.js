import { expect, test } from "bun:test";
import { createTree } from "./tree.js";

const { Document, Element, Text } = createTree();

test("inserts fragments as siblings and retains linked-tree identity", () => {
  const document = new Document();
  const root = document.createElement("main");
  const fragment = document.createDocumentFragment();
  const one = document.createElement("one");
  const two = document.createElement("two");
  fragment.append(one, two);
  document.appendChild(root);
  root.append("before", fragment, "after");

  expect(root.childNodes.map((node) => node.nodeName)).toEqual(["#text", "ONE", "TWO", "#text"]);
  expect(fragment.firstChild).toBeNull();
  expect(one.previousSibling).toBe(root.firstChild);
  expect(two.nextSibling).toBe(root.lastChild);
});

test("rejects hierarchy violations before mutating the tree", () => {
  const document = new Document();
  const root = document.createElement("main");
  const child = document.createElement("section");
  document.appendChild(root);
  root.appendChild(child);

  expect(() => child.appendChild(root)).toThrow("descendants");
  expect(root.parentNode).toBe(document);
  expect(child.parentNode).toBe(root);
  expect(() => document.appendChild(document.createElement("aside"))).toThrow("one document element");
});

test("adopts foreign nodes and clones attributes but not tree identity", () => {
  const left = new Document();
  const right = new Document();
  const element = left.createElement("card-item");
  element.setAttribute("data-state", "ready");
  element.appendChild(left.createTextNode("hello"));
  const target = right.createElement("main");
  right.appendChild(target);
  target.appendChild(element);

  expect(element.ownerDocument).toBe(right);
  expect(element.firstChild.ownerDocument).toBe(right);
  const copy = right.importNode(element, true);
  expect(copy).not.toBe(element);
  expect(copy.getAttribute("data-state")).toBe("ready");
  expect(copy.textContent).toBe("hello");
});

test("attributes are Attr nodes with ownership rules", () => {
  const document = new Document();
  const first = document.createElement("a");
  const second = document.createElement("b");
  const attribute = document.createAttribute("title");
  attribute.value = "hello";
  first.setAttributeNode(attribute);

  expect(first.attributes.item(0)).toBe(attribute);
  expect(() => second.setAttributeNode(attribute)).toThrow("already in use");
  expect(first.removeAttributeNode(attribute)).toBe(attribute);
  second.setAttributeNode(attribute);
  expect(second.getAttribute("title")).toBe("hello");
});

test("replaceChild retains the following sibling and imports attribute ownership", () => {
  const left = new Document();
  const right = new Document();
  const root = left.createElement("main");
  const first = left.createElement("first");
  const last = left.createElement("last");
  root.append(first, last);
  const replacement = right.createElement("replacement");
  replacement.setAttribute("role", "status");
  root.replaceChild(replacement, first);

  expect(root.childNodes).toEqual([replacement, last]);
  expect(replacement.nextSibling).toBe(last);
  expect(replacement.ownerDocument).toBe(left);
  expect(replacement.getAttributeNode("role").ownerDocument).toBe(left);
});

test("textContent replaces descendants and excludes comments", () => {
  const document = new Document();
  const root = document.createElement("main");
  root.append(new Text("one", document), document.createComment("ignored"), "two");
  expect(root.textContent).toBe("onetwo");
  root.textContent = "fresh";
  expect(root.childNodes).toHaveLength(1);
  expect(root.firstChild.data).toBe("fresh");
});
