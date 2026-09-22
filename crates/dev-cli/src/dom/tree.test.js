import { expect, test } from "runtime:test";
import { createTree } from "./tree.js";

const { CDATASection, CharacterData, Comment, DOMRect, Document, DocumentFragment, DocumentType, Element, HTMLDialogElement, HTMLInputElement, HTMLTableCellElement, HTMLTableRowElement, HTMLTableSectionElement, Node, ProcessingInstruction, SVGElement, Text, ValidityState, isDefined, setCurrentDocument } = createTree();

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

test("toggleAttribute follows presence and its optional force", () => {
  const document = new Document();
  const element = document.createElement("button");

  expect(element.toggleAttribute("disabled")).toBe(true);
  expect(element.getAttribute("disabled")).toBe("");
  expect(element.toggleAttribute("disabled")).toBe(false);
  expect(element.hasAttribute("disabled")).toBe(false);
  expect(element.toggleAttribute("disabled", true)).toBe(true);
  expect(element.toggleAttribute("disabled", true)).toBe(true);
  expect(element.toggleAttribute("disabled", false)).toBe(false);
  expect(element.hasAttribute("disabled")).toBe(false);
});

test("live NodeLists expose only their indexed own properties", () => {
  const document = new Document();
  const root = document.createElement("div");
  const nodes = root.childNodes;

  expect(Object.getOwnPropertyNames(nodes)).toEqual([]);
  root.append(document.createElement("i"), document.createElement("b"));
  expect(Object.getOwnPropertyNames(nodes)).toEqual(["0", "1"]);
  expect(Object.keys(nodes)).toEqual(["0", "1"]);
});

test("live HTMLCollections expose indexed and named properties", () => {
  const document = new Document();
  const root = document.createElement("div");
  const collection = root.getElementsByTagName("span");
  const first = document.createElement("span");
  const second = document.createElement("span");
  first.id = "first";
  second.setAttribute("name", "second");

  root.append(first, second);
  expect(collection.first).toBe(first);
  expect(collection.second).toBe(second);
  expect(Object.getOwnPropertyNames(collection)).toEqual(["0", "1", "first", "second"]);
  root.removeChild(first);
  expect(Object.getOwnPropertyNames(collection)).toEqual(["0", "second"]);
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

test("node lists iterate snapshots with forEach", () => {
  const document = new Document();
  const root = document.createElement("main");
  root.append(document.createElement("a"), document.createElement("b"));
  const seen = [];
  root.childNodes.forEach(function(node, index, list) {
    seen.push([this.prefix, node.localName, index, list === root.childNodes]);
  }, { prefix: "node" });

  expect(seen).toEqual([["node", "a", 0, true], ["node", "b", 1, true]]);
  expect(() => root.childNodes.forEach(null)).toThrow("function");
});

test("exposes template content as a same-object prototype accessor", () => {
  const document = new Document();
  const template = document.createElement("template");
  const fragment = template.content;

  expect(Object.hasOwn(template, "content")).toBe(false);
  expect("content" in Object.getPrototypeOf(template)).toBe(true);
  expect(fragment.constructor.name).toBe("DocumentFragment");
  expect(template.content).toBe(fragment);
  expect(() => { template.content = null; }).toThrow();
});

test("reads validity flags live from one ValidityState per control", () => {
  const document = new Document();
  const input = document.createElement("input");
  input.required = true;
  const validity = input.validity;

  expect(validity).toBeInstanceOf(ValidityState);
  expect(input.validity).toBe(validity);
  expect([validity.valueMissing, validity.valid]).toEqual([true, false]);
  input.value = "filled";
  expect([validity.valueMissing, validity.valid]).toEqual([false, true]);
  input.setCustomValidity("nope");
  expect([validity.customError, validity.valid]).toEqual([true, false]);
  expect(() => new ValidityState()).toThrow("Illegal constructor");
});

test("collects the controls a fieldset contains, live", () => {
  const document = new Document();
  const fieldset = document.createElement("fieldset");
  const root = document.createElement("main");
  document.appendChild(root);
  root.appendChild(fieldset);
  fieldset.append(document.createElement("input"), document.createElement("p"), document.createElement("select"));

  expect(Array.from(fieldset.elements, (control) => control.localName)).toEqual(["input", "select"]);
  fieldset.appendChild(document.createElement("textarea"));
  expect(fieldset.elements.length).toBe(3);
  fieldset.firstElementChild.remove();
  expect(Array.from(fieldset.elements, (control) => control.localName)).toEqual(["select", "textarea"]);
});

test("compares two trees by shape rather than by identity", () => {
  const document = new Document();
  const build = (className) => {
    const root = document.createElement("div");
    const child = document.createElement("p");
    child.setAttribute("class", className);
    child.appendChild(document.createTextNode("one"));
    root.appendChild(child);
    return root;
  };
  const left = build("x");

  expect(left.isEqualNode(build("x"))).toBe(true);
  expect(left.isEqualNode(build("y"))).toBe(false);
  expect(left.isEqualNode(left.cloneNode(true))).toBe(true);
  expect(left.isEqualNode(left.cloneNode(false))).toBe(false);
  expect(left.isSameNode(left)).toBe(true);
  expect(left.isSameNode(build("x"))).toBe(false);
  expect(document.createTextNode("t").isEqualNode(document.createTextNode("t"))).toBe(true);
  expect(document.createTextNode("t").isEqualNode(document.createComment("t"))).toBe(false);
});

test("reports tree order against the common ancestor", () => {
  const document = new Document();
  const root = document.createElement("main");
  const first = document.createElement("i");
  const second = document.createElement("b");
  root.append(first, second);
  document.appendChild(root);

  expect(root.compareDocumentPosition(root)).toBe(0);
  expect(root.compareDocumentPosition(first)).toBe(Node.DOCUMENT_POSITION_CONTAINED_BY | Node.DOCUMENT_POSITION_FOLLOWING);
  expect(first.compareDocumentPosition(root)).toBe(Node.DOCUMENT_POSITION_CONTAINS | Node.DOCUMENT_POSITION_PRECEDING);
  expect(first.compareDocumentPosition(second)).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
  expect(second.compareDocumentPosition(first)).toBe(Node.DOCUMENT_POSITION_PRECEDING);
  expect(document.createElement("i").compareDocumentPosition(first) & Node.DOCUMENT_POSITION_DISCONNECTED).toBe(Node.DOCUMENT_POSITION_DISCONNECTED);
  expect(() => root.compareDocumentPosition("nope")).toThrow("Node");
});

test("merges adjacent text nodes and drops empty ones", () => {
  const document = new Document();
  const root = document.createElement("p");
  const nested = document.createElement("i");
  nested.append(document.createTextNode("x"), document.createTextNode("y"));
  root.append(document.createTextNode("a"), document.createTextNode(""), document.createTextNode("b"), nested, document.createTextNode("c"));
  root.normalize();

  expect(Array.from(root.childNodes, (node) => node.nodeName)).toEqual(["#text", "I", "#text"]);
  expect(root.firstChild.data).toBe("ab");
  expect(nested.childNodes.length).toBe(1);
  expect(root.textContent).toBe("abxyc");
});

test("holds one doctype, before the document element", () => {
  const document = new Document();
  const doctype = document.implementation.createDocumentType("html");
  document.appendChild(doctype);
  document.appendChild(document.createElement("html"));

  expect([doctype.name, doctype.nodeType, doctype.nodeName, doctype.publicId, doctype.textContent]).toEqual(["html", 10, "html", "", null]);
  expect(document.doctype).toBe(doctype);
  expect(doctype).toBeInstanceOf(DocumentType);
  expect(() => document.appendChild(document.implementation.createDocumentType("html"))).toThrow("doctype");
  const fresh = new Document();
  fresh.appendChild(fresh.createElement("html"));
  expect(() => fresh.appendChild(fresh.implementation.createDocumentType("html"))).toThrow("precede");
});

test("creates whole HTML documents through the implementation", () => {
  const document = new Document();
  const made = document.implementation.createHTMLDocument("Made");

  expect(document.implementation).toBe(document.implementation);
  expect(made.doctype.name).toBe("html");
  expect(made.documentElement.tagName).toBe("HTML");
  expect(made.title).toBe("Made");
  expect(made.body.tagName).toBe("BODY");
  expect(made).not.toBe(document);
  made.title = "  Renamed\n  twice  ";
  expect([made.title, made.head.children.length]).toEqual(["Renamed twice", 1]);
});

test("makes processing instructions and refuses CDATA in HTML", () => {
  const document = new Document();
  const instruction = document.createProcessingInstruction("xml-stylesheet", 'href="x"');

  expect([instruction.target, instruction.data, instruction.nodeType]).toEqual(["xml-stylesheet", 'href="x"', 7]);
  expect(instruction).toBeInstanceOf(ProcessingInstruction);
  expect(instruction).toBeInstanceOf(CharacterData);
  expect(instruction.cloneNode().isEqualNode(instruction)).toBe(true);
  expect(() => document.createProcessingInstruction("bad name", "")).toThrow("target");
  expect(() => document.createProcessingInstruction("ok", "?>")).toThrow("?>");
  expect(() => document.createCDATASection("x")).toThrow("CDATA");
  expect(typeof CDATASection).toBe("function");
});

test("orders table rows by section rather than by position", () => {
  const document = new Document();
  const table = document.createElement("table");
  const head = document.createElement("thead");
  const foot = document.createElement("tfoot");
  const body = document.createElement("tbody");
  const row = (text) => {
    const element = document.createElement("tr");
    const cell = document.createElement("td");
    cell.appendChild(document.createTextNode(text));
    element.appendChild(cell);
    return element;
  };
  head.appendChild(row("h"));
  foot.appendChild(row("f"));
  body.append(row("a"), row("b"));
  // The foot is written before the body, and still comes last.
  table.append(head, foot, body);
  document.appendChild(table);

  expect(Array.from(table.rows, (entry) => entry.textContent)).toEqual(["h", "a", "b", "f"]);
  expect([table.tBodies.length, table.tHead, table.tFoot]).toEqual([1, head, foot]);
  expect(body.rows.length).toBe(2);
  expect(table.rows[1]).toBeInstanceOf(HTMLTableRowElement);
  expect(body).toBeInstanceOf(HTMLTableSectionElement);
});

test("indexes rows and cells against the tree they are in", () => {
  const document = new Document();
  const table = document.createElement("table");
  const body = document.createElement("tbody");
  const row = document.createElement("tr");
  const first = document.createElement("td");
  const second = document.createElement("th");
  row.append(first, second);
  body.appendChild(row);
  table.appendChild(body);
  document.appendChild(table);

  expect([row.cells.length, row.rowIndex, row.sectionRowIndex]).toEqual([2, 0, 0]);
  expect([first.cellIndex, second.cellIndex]).toEqual([0, 1]);
  expect(second).toBeInstanceOf(HTMLTableCellElement);
  expect([document.createElement("tr").rowIndex, document.createElement("td").cellIndex]).toEqual([-1, -1]);
});

test("answers geometry with zeros instead of throwing", () => {
  const document = new Document();
  const element = document.createElement("div");
  document.appendChild(element);
  const rect = element.getBoundingClientRect();

  expect(rect).toBeInstanceOf(DOMRect);
  expect([rect.x, rect.y, rect.width, rect.height, rect.top, rect.right, rect.bottom, rect.left]).toEqual([0, 0, 0, 0, 0, 0, 0, 0]);
  expect(rect.toJSON().width).toBe(0);
  expect([element.offsetWidth, element.offsetHeight, element.clientWidth, element.scrollHeight]).toEqual([0, 0, 0, 0]);
  expect(element.getClientRects().length).toBe(0);
  // Assignable and still zero, as for any element a browser cannot scroll.
  element.scrollTop = 40;
  expect([element.scrollTop, element.scrollLeft]).toEqual([0, 0]);
  expect(element.scrollIntoView()).toBeUndefined();
  expect(new DOMRect(1, 2, 3, 4).right).toBe(4);
});

test("lowercases an HTML element name and keeps a namespaced one", () => {
  const document = new Document();

  expect(document.createElement("DIV").localName).toBe("div");
  expect(document.createElement("DIV").tagName).toBe("DIV");
  expect(document.createElement("Input").localName).toBe("input");
  expect(document.createElementNS("http://www.w3.org/2000/svg", "linearGradient").localName).toBe("linearGradient");
  expect(() => document.createElement("1bad")).toThrow("valid HTML names");
});

test("a constructed node belongs to the current document", () => {
  // The window names it; with no window the first document made is it, so this
  // case says which one it means rather than depending on test order.
  const document = setCurrentDocument(new Document());
  const root = document.createElement("main");
  document.appendChild(root);

  const fragment = new DocumentFragment();
  expect(fragment.ownerDocument).toBe(document);
  fragment.append(new Text("one"), new Comment("two"));
  root.appendChild(fragment);
  expect(Array.from(root.childNodes, (node) => node.nodeName)).toEqual(["#text", "#comment"]);
  expect(root.firstChild.ownerDocument).toBe(document);
  expect(new Text().data).toBe("");
});

test("a fragment and a shadow root answer getElementById", () => {
  const document = setCurrentDocument(new Document());
  const fragment = new DocumentFragment();
  const inside = document.createElement("b");
  inside.id = "in-fragment";
  fragment.appendChild(inside);

  expect(fragment.getElementById("in-fragment")).toBe(inside);
  expect(fragment.getElementById("absent")).toBeNull();
  const host = document.createElement("div");
  document.appendChild(host);
  const root = host.attachShadow({ mode: "open" });
  const scoped = document.createElement("p");
  scoped.id = "in-shadow";
  root.appendChild(scoped);
  expect(root.getElementById("in-shadow")).toBe(scoped);
  // The id is the shadow tree's own: the document does not see it.
  expect(document.getElementById("in-shadow")).toBeNull();
});

test("a radio group holds one checked button, however it was checked", () => {
  const document = setCurrentDocument(new Document());
  const form = document.createElement("form");
  document.appendChild(form);
  const radio = (name) => {
    const input = document.createElement("input");
    input.type = "radio";
    if (name) input.name = name;
    form.appendChild(input);
    return input;
  };
  const first = radio("pick");
  const second = radio("pick");
  const elsewhere = radio("other");

  first.checked = true;
  second.checked = true;
  expect([first.checked, second.checked, elsewhere.checked]).toEqual([false, true, false]);
  first.checked = true;
  expect([first.checked, second.checked]).toEqual([true, false]);
  // Unchecking is not exclusive, and another name is another group.
  elsewhere.checked = true;
  first.checked = false;
  expect([first.checked, second.checked, elsewhere.checked]).toEqual([false, false, true]);
  // A radio with no name is in no group.
  const nameless = radio("");
  const another = radio("");
  nameless.checked = true;
  another.checked = true;
  expect([nameless.checked, another.checked]).toEqual([true, true]);
});

test("reflects the ARIA mixin to its attributes", () => {
  const document = setCurrentDocument(new Document());
  const element = document.createElement("div");

  expect(element.ariaLabel).toBeNull();
  element.role = "button";
  element.ariaLabel = "Save";
  element.ariaValueMax = "9";
  expect([element.getAttribute("role"), element.getAttribute("aria-label"), element.getAttribute("aria-valuemax")])
    .toEqual(["button", "Save", "9"]);
  element.setAttribute("aria-label", "Changed");
  expect(element.ariaLabel).toBe("Changed");
  element.ariaLabel = null;
  expect(element.hasAttribute("aria-label")).toBe(false);
});

test("refuses a shadow root on an element that cannot host one", () => {
  const document = setCurrentDocument(new Document());
  const hosts = ["div", "span", "section", "p"].map((name) => document.createElement(name).attachShadow({ mode: "open" }));

  expect(hosts.every((root) => root.mode === "open")).toBe(true);
  expect(() => document.createElement("input").attachShadow({ mode: "open" })).toThrow("cannot host");
  expect(() => document.createElement("li").attachShadow({ mode: "open" })).toThrow("cannot host");
  // A custom element name always may.
  expect(document.createElement("x-thing").attachShadow({ mode: "open" }).mode).toBe("open");
});

test("a template's content is inert until it is cloned into a tree", () => {
  const document = setCurrentDocument(new Document());
  const template = document.createElement("template");
  const inside = document.createElement("x-inert");
  template.content.appendChild(inside);

  expect(inside.getRootNode()).toBe(template.content);
  expect(isDefined(inside)).toBe(false);
  const loose = document.createElement("x-inert");
  expect(isDefined(loose)).toBe(false);
  // A plain element is defined by being built in, wherever it sits.
  expect(isDefined(document.createElement("div"))).toBe(true);
});
