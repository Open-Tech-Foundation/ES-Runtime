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

  expect(Array.from(root.childNodes, (node) => node.nodeName)).toEqual(["#text", "ONE", "TWO", "#text"]);
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

test("named node maps expose live numeric attribute entries", () => {
  const document = new Document();
  const element = document.createElement("a");
  const attributes = element.attributes;
  element.setAttribute("first", "one");
  element.setAttribute("second", "two");

  expect(attributes[0].name).toBe("first");
  expect(attributes[1].value).toBe("two");
  expect(attributes[2]).toBeUndefined();
  element.removeAttribute("first");
  expect(attributes[0].name).toBe("second");
  expect(attributes.item(1)).toBeNull();
});

test("documents return the first matching element by ID in tree order", () => {
  const document = new Document();
  const root = document.createElement("main");
  const first = document.createElement("a");
  const second = document.createElement("b");
  first.id = "duplicate";
  second.id = "duplicate";
  root.append(first, second);
  document.appendChild(root);

  expect(document.getElementById("duplicate")).toBe(first);
  first.remove();
  expect(document.getElementById("duplicate")).toBe(second);
  expect(document.getElementById("missing")).toBeNull();
});

test("class lists are live unique token collections", () => {
  const document = new Document();
  const element = document.createElement("a");
  const classes = element.classList;
  element.className = "one one two";

  expect(classes).toBe(element.classList);
  expect(Array.from(classes)).toEqual(["one", "two"]);
  expect(classes[1]).toBe("two");
  expect(classes.item(2)).toBeNull();
  classes.add("three", "one");
  classes.remove("two");
  expect(element.getAttribute("class")).toBe("one three");
  expect(classes.replace("one", "first")).toBe(true);
  expect(classes.toggle("three")).toBe(false);
  expect(classes.toggle("four", true)).toBe(true);
  expect(element.className).toBe("first four");
});

test("class list token validation happens before mutations", () => {
  const document = new Document();
  const element = document.createElement("a");
  element.className = "ready";

  expect(() => element.classList.add("next", "bad token")).toThrow("whitespace");
  expect(() => element.classList.contains("")).toThrow("empty");
  expect(element.className).toBe("ready");
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

  expect(Array.from(root.childNodes)).toEqual([replacement, last]);
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

test("node lists and HTML collections are live, indexed, and named", () => {
  const document = new Document();
  const root = document.createElement("main");
  const childNodes = root.childNodes;
  const children = root.children;
  const cards = document.getElementsByClassName("card selected");
  document.appendChild(root);
  const card = document.createElement("article");
  card.id = "primary";
  card.className = "card selected";
  root.append("text", card);

  expect(root.childNodes).toBe(childNodes);
  expect(root.children).toBe(children);
  expect(childNodes.length).toBe(2);
  expect(childNodes[1]).toBe(card);
  expect(children.item(0)).toBe(card);
  expect(cards.length).toBe(1);
  expect(cards.namedItem("primary")).toBe(card);
  card.className = "card";
  expect(cards.length).toBe(0);
  root.removeChild(card);
  expect(children.length).toBe(0);
});
