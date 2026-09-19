// The mutable core of esdev's test-only DOM. It is a factory rather than a
// global installer so the test runner can create one isolated realm per file.
// Public classes keep state in symbols: framework code inspecting an element
// sees the DOM surface, not the linked-list bookkeeping behind it.

const SLOT = Symbol("esdev DOM slots");
const ATTRS = Symbol("esdev DOM attributes");
const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);

function domError(name, message) {
  return new DOMException(message, name);
}

function slots(node) {
  return node[SLOT];
}

function descendants(node, visitor) {
  visitor(node);
  for (const attribute of slots(node).attributes ?? []) visitor(attribute);
  for (let child = node.firstChild; child; child = child.nextSibling) descendants(child, visitor);
}

function isInclusiveAncestor(ancestor, node) {
  for (let current = node; current; current = current.parentNode) {
    if (current === ancestor) return true;
  }
  return false;
}

function asNodes(value, document, NodeClass) {
  return value.map((item) => (item instanceof NodeClass ? item : document.createTextNode(String(item))));
}

export function createTree() {
  class Node {
    static ELEMENT_NODE = 1;
    static ATTRIBUTE_NODE = 2;
    static TEXT_NODE = 3;
    static COMMENT_NODE = 8;
    static DOCUMENT_NODE = 9;
    static DOCUMENT_FRAGMENT_NODE = 11;

    constructor(type, name, ownerDocument) {
      Object.defineProperty(this, SLOT, {
        value: { type, name, ownerDocument, parent: null, first: null, last: null, previous: null, next: null },
      });
    }

    get nodeType() { return slots(this).type; }
    get nodeName() { return slots(this).name; }
    get ownerDocument() { return slots(this).ownerDocument; }
    get parentNode() { return slots(this).parent; }
    get firstChild() { return slots(this).first; }
    get lastChild() { return slots(this).last; }
    get previousSibling() { return slots(this).previous; }
    get nextSibling() { return slots(this).next; }
    get parentElement() { return this.parentNode instanceof Element ? this.parentNode : null; }
    get childNodes() { return Array.from(this._children()); }
    get isConnected() { return this.getRootNode() instanceof Document; }

    *_children() {
      for (let child = this.firstChild; child; child = child.nextSibling) yield child;
    }

    getRootNode() {
      let root = this;
      while (root.parentNode) root = root.parentNode;
      return root;
    }

    hasChildNodes() { return this.firstChild !== null; }

    appendChild(node) { return this.insertBefore(node, null); }

    insertBefore(node, child) {
      if (!(node instanceof Node)) throw new TypeError("insertBefore expects a Node");
      if (child !== null && child.parentNode !== this) throw domError("NotFoundError", "The reference child is not a child of this node.");
      this._preInsert(node, child);
      return node;
    }

    replaceChild(node, child) {
      if (!(node instanceof Node)) throw new TypeError("replaceChild expects a Node");
      if (child.parentNode !== this) throw domError("NotFoundError", "The child is not a child of this node.");
      if (node === child) return child;
      const reference = child.nextSibling;
      this._validateInsertion(node, reference, child);
      this._remove(child);
      this._insert(node, reference);
      return child;
    }

    removeChild(child) {
      if (child.parentNode !== this) throw domError("NotFoundError", "The child is not a child of this node.");
      this._remove(child);
      return child;
    }

    remove() {
      if (this.parentNode) this.parentNode.removeChild(this);
    }

    before(...items) {
      if (!this.parentNode) return;
      this.parentNode._insertMany(asNodes(items, this.ownerDocument, Node), this);
    }

    after(...items) {
      if (!this.parentNode) return;
      this.parentNode._insertMany(asNodes(items, this.ownerDocument, Node), this.nextSibling);
    }

    replaceWith(...items) {
      if (!this.parentNode) return;
      const parent = this.parentNode;
      parent._insertMany(asNodes(items, this.ownerDocument, Node), this);
      parent.removeChild(this);
    }

    append(...items) { this._insertMany(asNodes(items, this.ownerDocument ?? this, Node), null); }
    prepend(...items) { this._insertMany(asNodes(items, this.ownerDocument ?? this, Node), this.firstChild); }

    _insertMany(nodes, before) {
      for (const node of nodes) this._preInsert(node, before);
    }

    _preInsert(node, before) {
      this._validateInsertion(node, before, null);
      this._insert(node, before);
    }

    _validateInsertion(node, before, replacing) {
      if (!(this instanceof Document || this instanceof DocumentFragment || this instanceof Element)) {
        throw domError("HierarchyRequestError", "This node cannot have children.");
      }
      if (isInclusiveAncestor(node, this)) {
        throw domError("HierarchyRequestError", "A node cannot be inserted into one of its descendants.");
      }
      const candidates = node instanceof DocumentFragment ? Array.from(node._children()) : [node];
      for (const candidate of candidates) {
        if (candidate instanceof Document || candidate instanceof Attr) {
          throw domError("HierarchyRequestError", "Document and attribute nodes cannot be inserted here.");
        }
      }
      if (this instanceof Document) {
        const elements = Array.from(this._children()).filter((node) => node instanceof Element && node !== replacing);
        const incoming = candidates.filter((node) => node instanceof Element);
        if (candidates.some((node) => node instanceof Text && node.data.trim() !== "")) {
          throw domError("HierarchyRequestError", "A document cannot have text-node children.");
        }
        if (elements.length + incoming.length > 1) {
          throw domError("HierarchyRequestError", "A document can have only one document element.");
        }
      }
      if (before !== null && before.parentNode !== this) throw domError("NotFoundError", "The reference child is not a child of this node.");
    }

    _insert(node, before) {
      const document = this instanceof Document ? this : this.ownerDocument;
      const candidates = node instanceof DocumentFragment ? Array.from(node._children()) : [node];
      for (const candidate of candidates) {
        if (candidate.ownerDocument !== document) document.adoptNode(candidate);
        if (candidate.parentNode) candidate.parentNode._remove(candidate);
        const target = slots(this);
        const next = before;
        const previous = next ? next.previousSibling : target.last;
        const state = slots(candidate);
        state.parent = this;
        state.previous = previous;
        state.next = next;
        if (previous) slots(previous).next = candidate; else target.first = candidate;
        if (next) slots(next).previous = candidate; else target.last = candidate;
      }
    }

    _remove(child) {
      const state = slots(child);
      const parent = slots(this);
      if (state.previous) slots(state.previous).next = state.next; else parent.first = state.next;
      if (state.next) slots(state.next).previous = state.previous; else parent.last = state.previous;
      state.parent = null;
      state.previous = null;
      state.next = null;
    }

    get textContent() {
      if (this instanceof Text || this instanceof Comment) return this.data;
      if (this instanceof Attr) return this.value;
      let text = "";
      for (const child of this._children()) {
        if (!(child instanceof Comment)) text += child.textContent;
      }
      return text;
    }

    set textContent(value) {
      if (this instanceof Text || this instanceof Comment) { this.data = value ?? ""; return; }
      if (this instanceof Attr) { this.value = value ?? ""; return; }
      while (this.firstChild) this._remove(this.firstChild);
      if (value !== null && value !== "") this.appendChild((this.ownerDocument ?? this).createTextNode(String(value)));
    }

    cloneNode(deep = false) {
      const document = this.ownerDocument ?? this;
      let clone;
      if (this instanceof Document) clone = new Document();
      else if (this instanceof DocumentFragment) clone = document.createDocumentFragment();
      else if (this instanceof Element) {
        clone = document.createElement(this.localName);
        for (const attribute of this.attributes) clone.setAttribute(attribute.name, attribute.value);
      } else if (this instanceof Text) clone = document.createTextNode(this.data);
      else if (this instanceof Comment) clone = document.createComment(this.data);
      else throw domError("NotSupportedError", "This node cannot be cloned.");
      if (deep) for (const child of this._children()) clone.appendChild(child.cloneNode(true));
      return clone;
    }
  }

  class CharacterData extends Node {
    constructor(type, name, data, ownerDocument) { super(type, name, ownerDocument); this.data = String(data); }
    get nodeValue() { return this.data; }
    set nodeValue(value) { this.data = String(value ?? ""); }
  }

  class Text extends CharacterData {
    constructor(data, ownerDocument) { super(Node.TEXT_NODE, "#text", data, ownerDocument); }
  }

  class Comment extends CharacterData {
    constructor(data, ownerDocument) { super(Node.COMMENT_NODE, "#comment", data, ownerDocument); }
  }

  class Attr extends Node {
    constructor(name, value, ownerDocument) {
      super(Node.ATTRIBUTE_NODE, name, ownerDocument);
      this.name = name;
      this.value = String(value);
      this.ownerElement = null;
    }
    get nodeValue() { return this.value; }
    set nodeValue(value) { this.value = String(value ?? ""); }
  }

  class NamedNodeMap {
    constructor(element) { Object.defineProperty(this, ATTRS, { value: element }); }
    _list() { return slots(this[ATTRS]).attributes; }
    get length() { return this._list().length; }
    item(index) { return this._list()[index] ?? null; }
    getNamedItem(name) { return this._list().find((attribute) => attribute.name === String(name)) ?? null; }
    setNamedItem(attribute) {
      if (!(attribute instanceof Attr)) throw new TypeError("setNamedItem expects an Attr");
      const existing = this.getNamedItem(attribute.name);
      if (attribute.ownerElement && attribute.ownerElement !== this[ATTRS]) throw domError("InUseAttributeError", "The attribute is already in use.");
      if (existing) this._list()[this._list().indexOf(existing)] = attribute; else this._list().push(attribute);
      attribute.ownerElement = this[ATTRS];
      if (existing) existing.ownerElement = null;
      return existing;
    }
    removeNamedItem(name) {
      const attribute = this.getNamedItem(name);
      if (!attribute) throw domError("NotFoundError", "No such attribute.");
      this._list().splice(this._list().indexOf(attribute), 1);
      attribute.ownerElement = null;
      return attribute;
    }
    [Symbol.iterator]() { return this._list()[Symbol.iterator](); }
  }

  class Element extends Node {
    constructor(name, ownerDocument) {
      super(Node.ELEMENT_NODE, name.toUpperCase(), ownerDocument);
      this.localName = name;
      this.tagName = name.toUpperCase();
      slots(this).attributes = [];
      Object.defineProperty(this, "attributes", { value: new NamedNodeMap(this) });
    }
    getAttribute(name) { return this.attributes.getNamedItem(String(name))?.value ?? null; }
    getAttributeNode(name) { return this.attributes.getNamedItem(String(name)); }
    hasAttribute(name) { return this.getAttributeNode(name) !== null; }
    setAttribute(name, value) { this.attributes.setNamedItem(new Attr(String(name), value, this.ownerDocument)); }
    setAttributeNode(attribute) { return this.attributes.setNamedItem(attribute); }
    removeAttribute(name) { const attribute = this.getAttributeNode(name); if (attribute) this.attributes.removeNamedItem(name); }
    removeAttributeNode(attribute) { if (attribute.ownerElement !== this) throw domError("NotFoundError", "The attribute is not owned by this element."); return this.attributes.removeNamedItem(attribute.name); }
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get children() { return Array.from(this._children()).filter((node) => node instanceof Element); }
  }

  class DocumentFragment extends Node {
    constructor(ownerDocument) { super(Node.DOCUMENT_FRAGMENT_NODE, "#document-fragment", ownerDocument); }
  }

  class Document extends Node {
    constructor() {
      super(Node.DOCUMENT_NODE, "#document", null);
      slots(this).ownerDocument = this;
    }
    get documentElement() { return Array.from(this._children()).find((node) => node instanceof Element) ?? null; }
    createElement(name) {
      name = String(name);
      if (!/^[a-z][a-z0-9_:-]*$/.test(name)) throw domError("InvalidCharacterError", "Element names must be lowercase modern HTML names.");
      return new Element(name, this);
    }
    createTextNode(data) { return new Text(data, this); }
    createComment(data) { return new Comment(data, this); }
    createDocumentFragment() { return new DocumentFragment(this); }
    createAttribute(name) { return new Attr(String(name), "", this); }
    adoptNode(node) {
      if (!(node instanceof Node) || node instanceof Document) throw domError("NotSupportedError", "A document cannot be adopted.");
      if (node.parentNode) node.parentNode.removeChild(node);
      descendants(node, (item) => { slots(item).ownerDocument = this; });
      return node;
    }
    importNode(node, deep = false) {
      if (!(node instanceof Node) || node instanceof Document) throw domError("NotSupportedError", "A document cannot be imported.");
      const copy = node.cloneNode(deep);
      descendants(copy, (item) => { slots(item).ownerDocument = this; });
      return copy;
    }
  }

  return { Node, Document, DocumentFragment, Element, Text, Comment, Attr, NamedNodeMap, VOID };
}
