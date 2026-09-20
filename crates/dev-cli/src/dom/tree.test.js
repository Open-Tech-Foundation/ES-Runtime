import { expect, test } from "bun:test";
import { createTree } from "./tree.js";

const { Document, Element, HTMLDialogElement, HTMLInputElement, SVGElement, Text } = createTree();

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

test("namespace attribute access treats null namespaces as ordinary attributes", () => {
  const document = new Document();
  const element = document.createElement("a");
  element.setAttribute("title", "first");

  expect(element.getAttributeNS(null, "title")).toBe("first");
  expect(element.hasAttributeNS(null, "title")).toBe(true);
  element.setAttributeNS(null, "title", "second");
  expect(element.getAttribute("title")).toBe("second");
  element.removeAttributeNS(null, "title");
  expect(element.hasAttribute("title")).toBe(false);
});

test("namespace attribute operations distinguish local names and preserve clones", () => {
  const document = new Document();
  const element = document.createElement("use");
  const xlink = "http://www.w3.org/1999/xlink";
  element.setAttributeNS(xlink, "xlink:href", "#first");
  element.setAttributeNS("urn:example", "example:href", "#second");

  expect(element.getAttributeNS(xlink, "href")).toBe("#first");
  expect(element.attributes.getNamedItemNS("urn:example", "href").prefix).toBe("example");
  element.setAttributeNS(xlink, "xlink:href", "#next");
  expect(element.attributes.length).toBe(2);
  const clone = element.cloneNode();
  expect(clone.getAttributeNS(xlink, "href")).toBe("#next");
  element.removeAttributeNS(xlink, "href");
  expect(element.hasAttributeNS(xlink, "href")).toBe(false);
  expect(element.hasAttributeNS("urn:example", "href")).toBe(true);
});

test("ordinary colon attributes retain their complete local name", () => {
  const document = new Document();
  const element = document.createElement("use");
  element.setAttribute("xlink:href", "#first");

  expect(element.getAttributeNS(null, "xlink:href")).toBe("#first");
  expect(element.hasAttributeNS(null, "href")).toBe(false);
});

test("createElementNS preserves modern namespace identity", () => {
  const document = new Document();
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  const use = document.createElementNS("http://www.w3.org/2000/svg", "xlink:use");
  const input = document.createElementNS("http://www.w3.org/1999/xhtml", "input");

  expect([svg.namespaceURI, svg.nodeName, use.prefix, use.localName]).toEqual(["http://www.w3.org/2000/svg", "svg", "xlink", "use"]);
  expect(svg).toBeInstanceOf(SVGElement);
  expect(input).toBeInstanceOf(HTMLInputElement);
  input.value = "modern";
  expect(input.value).toBe("modern");
});

test("DOM internals do not collide with framework child bookkeeping", () => {
  const document = new Document();
  const element = document.createElement("div");
  element.append(document.createElement("span"));
  element._children = { framework: true };

  expect(element.childNodes.length).toBe(1);
  expect(element.children.item(0).localName).toBe("span");
});

test("dialog open reflects as a boolean attribute", () => {
  const document = new Document();
  const dialog = document.createElement("dialog");

  expect(dialog).toBeInstanceOf(HTMLDialogElement);
  dialog.open = true;
  expect([dialog.open, dialog.getAttribute("open")]).toEqual([true, ""]);
  dialog.open = false;
  expect([dialog.open, dialog.hasAttribute("open")]).toEqual([false, false]);
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

test("datasets stay live with data attributes and enumerate property names", () => {
  const document = new Document();
  const element = document.createElement("article");
  const dataset = element.dataset;
  element.setAttribute("data-user-id", "first");
  element.setAttribute("data-ready", "");

  expect(dataset).toBe(element.dataset);
  expect(dataset.userId).toBe("first");
  expect(Object.keys(dataset)).toEqual(["userId", "ready"]);
  dataset.userId = 42;
  expect(element.getAttribute("data-user-id")).toBe("42");
  delete dataset.ready;
  expect(element.hasAttribute("data-ready")).toBe(false);
});

test("datasets use HTML name conversion and reject unrepresentable property names", () => {
  const document = new Document();
  const element = document.createElement("article");
  element.dataset.recordId = "one";
  element.setAttribute("data--leading", "two");

  expect(element.getAttribute("data-record-id")).toBe("one");
  expect(element.dataset.Leading).toBe("two");
  expect(() => { element.dataset["record-id"] = "no"; }).toThrow("hyphen");
  expect(element.hasAttribute("data-record-id")).toBe(true);
});

test("input indeterminate state defaults to false and does not reflect an attribute", () => {
  const document = new Document();
  const input = document.createElement("input");
  input.type = "checkbox";

  expect(input.indeterminate).toBe(false);
  input.indeterminate = 1;
  expect(input.indeterminate).toBe(true);
  expect(input.hasAttribute("indeterminate")).toBe(false);
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

test("contains includes the receiver and follows only descendant links", () => {
  const document = new Document();
  const root = document.createElement("main");
  const child = document.createElement("article");
  const detached = document.createElement("aside");
  document.appendChild(root);
  root.appendChild(child);

  expect(document.contains(document)).toBe(true);
  expect(document.contains(child)).toBe(true);
  expect(root.contains(child)).toBe(true);
  expect(child.contains(root)).toBe(false);
  expect(root.contains(detached)).toBe(false);
  expect(root.contains(null)).toBe(false);
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
  expect(1 in childNodes).toBe(true);
  expect(Array.prototype.slice.call(childNodes)).toEqual([root.firstChild, card]);
  expect(children.item(0)).toBe(card);
  expect(cards.length).toBe(1);
  expect(cards.namedItem("primary")).toBe(card);
  card.className = "card";
  expect(cards.length).toBe(0);
  root.removeChild(card);
  expect(children.length).toBe(0);
});
