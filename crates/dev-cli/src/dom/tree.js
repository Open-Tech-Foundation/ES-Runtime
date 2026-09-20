// The mutable core of esdev's test-only DOM. It is a factory rather than a
// global installer so the test runner can create one isolated realm per file.
// Public classes keep state in symbols: framework code inspecting an element
// sees the DOM surface, not the linked-list bookkeeping behind it.

const SLOT = Symbol("esdev DOM slots");
const ATTRS = Symbol("esdev DOM attributes");
const CLASS_LIST = Symbol("esdev DOM class list");
const DATASET = Symbol("esdev DOM dataset");
const DATA = Symbol("esdev DOM character data");
const SELECTED = Symbol("esdev DOM option selected state");
const TEXTAREA_VALUE = Symbol("esdev DOM textarea value state");
const CUSTOM_VALIDITY = Symbol("esdev DOM custom validity");
const INPUT_VALUE = Symbol("esdev DOM input value state");
const INPUT_CHECKED = Symbol("esdev DOM input checked state");
const INPUT_INDETERMINATE = Symbol("esdev DOM input indeterminate state");
const SHADOW_ROOT = Symbol("esdev DOM shadow root");
const HTML_NAMESPACE = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE = "http://www.w3.org/1998/Math/MathML";
const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);

function domError(name, message) {
  return new DOMException(message, name);
}

function slots(node) {
  return node[SLOT];
}

function childIndex(node) {
  let index = 0;
  for (let sibling = node.previousSibling; sibling; sibling = sibling.previousSibling) index += 1;
  return index;
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

export function createTree(events = {}) {
  const { EventTarget = class {}, Event = class {}, MouseEvent = class {}, SubmitEvent = class {} } = events;
  const customConstruction = [];
  class LiveCollection {
    constructor(root, filter) {
      this.root = root;
      this.filter = filter;
      this.version = -1;
      this.values = [];
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target._values()[Number(property)];
          return Reflect.get(target, property, receiver);
        },
        has(target, property) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return Number(property) < target._values().length;
          return Reflect.has(target, property);
        },
      });
    }
    _values() {
      const document = this.root.ownerDocument ?? this.root;
      const version = slots(document).version;
      if (this.version !== version) {
        this.values = this.filter(this.root);
        this.version = version;
      }
      return this.values;
    }
    get length() { return this._values().length; }
    item(index) { return this._values()[index] ?? null; }
    [Symbol.iterator]() { return this._values()[Symbol.iterator](); }
  }

  class NodeList extends LiveCollection {}
  class HTMLCollection extends LiveCollection {
    namedItem(name) {
      name = String(name);
      return this._values().find((element) => element.id === name || element.getAttribute("name") === name) ?? null;
    }
  }

  class DOMTokenList {
    constructor(element) {
      Object.defineProperty(this, ATTRS, { value: element });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target._tokens()[Number(property)];
          return Reflect.get(target, property, receiver);
        },
      });
    }
    _tokens() {
      return [...new Set((this[ATTRS].getAttribute("class") ?? "").trim().split(/\s+/).filter(Boolean))];
    }
    _set(tokens) { this[ATTRS].setAttribute("class", tokens.join(" ")); }
    _validate(token) {
      token = String(token);
      if (token === "") throw domError("SyntaxError", "The token must not be empty.");
      if (/\s/.test(token)) throw domError("InvalidCharacterError", "The token must not contain ASCII whitespace.");
      return token;
    }
    get length() { return this._tokens().length; }
    get value() { return this[ATTRS].getAttribute("class") ?? ""; }
    set value(value) { this[ATTRS].setAttribute("class", String(value)); }
    item(index) { return this._tokens()[Number(index)] ?? null; }
    contains(token) { return this._tokens().includes(this._validate(token)); }
    add(...tokens) {
      tokens = tokens.map((token) => this._validate(token));
      const next = this._tokens();
      for (const token of tokens) if (!next.includes(token)) next.push(token);
      this._set(next);
    }
    remove(...tokens) {
      tokens = new Set(tokens.map((token) => this._validate(token)));
      this._set(this._tokens().filter((token) => !tokens.has(token)));
    }
    toggle(token, force) {
      token = this._validate(token);
      const has = this._tokens().includes(token);
      const add = force === undefined ? !has : Boolean(force);
      if (add && !has) this.add(token);
      if (!add && has) this.remove(token);
      return add;
    }
    replace(token, replacement) {
      token = this._validate(token);
      replacement = this._validate(replacement);
      const tokens = this._tokens();
      const index = tokens.indexOf(token);
      if (index === -1) return false;
      tokens[index] = replacement;
      this._set([...new Set(tokens)]);
      return true;
    }
    [Symbol.iterator]() { return this._tokens()[Symbol.iterator](); }
  }

  function datasetProperty(name) {
    if (!name.startsWith("data-")) return null;
    return name.slice(5).replace(/-([a-z])/g, (_, character) => character.toUpperCase());
  }

  function datasetAttribute(property) {
    property = String(property);
    if (/-[a-z]/.test(property)) return null;
    return `data-${property.replace(/[A-Z]/g, (character) => `-${character.toLowerCase()}`)}`;
  }

  class DOMStringMap {
    constructor(element) {
      Object.defineProperty(this, ATTRS, { value: element });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string") {
            const attribute = datasetAttribute(property);
            if (attribute && target[ATTRS].hasAttribute(attribute)) return target[ATTRS].getAttribute(attribute);
          }
          if (property === Symbol.toStringTag) return "DOMStringMap";
          return Reflect.get(target, property, receiver);
        },
        set(target, property, value, receiver) {
          if (typeof property !== "string") return Reflect.set(target, property, value, receiver);
          const attribute = datasetAttribute(property);
          if (!attribute) throw domError("SyntaxError", "Dataset property names must not contain a hyphen followed by a lowercase letter.");
          target[ATTRS].setAttribute(attribute, String(value));
          return true;
        },
        has(target, property) {
          if (typeof property === "string") {
            const attribute = datasetAttribute(property);
            if (attribute && target[ATTRS].hasAttribute(attribute)) return true;
          }
          return Reflect.has(target, property);
        },
        deleteProperty(target, property) {
          if (typeof property !== "string") return Reflect.deleteProperty(target, property);
          const attribute = datasetAttribute(property);
          if (!attribute) return true;
          target[ATTRS].removeAttribute(attribute);
          return true;
        },
        ownKeys(target) {
          return [...new Set([
            ...Reflect.ownKeys(target),
            ...Array.from(target[ATTRS].attributes, (attribute) => datasetProperty(attribute.name)).filter((property) => property !== null),
          ])];
        },
        getOwnPropertyDescriptor(target, property) {
          if (typeof property !== "string") return Reflect.getOwnPropertyDescriptor(target, property);
          const attribute = datasetAttribute(property);
          if (!attribute || !target[ATTRS].hasAttribute(attribute)) return undefined;
          return { configurable: true, enumerable: true, writable: true, value: target[ATTRS].getAttribute(attribute) };
        },
      });
    }
  }

  class Node extends EventTarget {
    static ELEMENT_NODE = 1;
    static ATTRIBUTE_NODE = 2;
    static TEXT_NODE = 3;
    static COMMENT_NODE = 8;
    static DOCUMENT_NODE = 9;
    static DOCUMENT_FRAGMENT_NODE = 11;

    constructor(type, name, ownerDocument) {
      super();
      Object.defineProperty(this, SLOT, {
        value: { type, name, ownerDocument, parent: null, first: null, last: null, previous: null, next: null, childNodes: null },
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
    get previousElementSibling() {
      for (let sibling = this.previousSibling; sibling; sibling = sibling.previousSibling) if (sibling instanceof Element) return sibling;
      return null;
    }
    get nextElementSibling() {
      for (let sibling = this.nextSibling; sibling; sibling = sibling.nextSibling) if (sibling instanceof Element) return sibling;
      return null;
    }
    get parentElement() { return this.parentNode instanceof Element ? this.parentNode : null; }
    get childNodes() {
      const state = slots(this);
      return state.childNodes ??= new NodeList(this, (root) => Array.from(root._esdevChildren()));
    }
    get isConnected() { return this.getRootNode({ composed: true }) instanceof Document; }

    *_esdevChildren() {
      for (let child = this.firstChild; child; child = child.nextSibling) yield child;
    }

    getRootNode(options = {}) {
      let root = this;
      while (root.parentNode || options.composed && root instanceof ShadowRoot) root = root.parentNode ?? root.host;
      return root;
    }

    _eventParent(event) { return this.parentNode; }

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
    replaceChildren(...items) {
      const nodes = asNodes(items, this.ownerDocument ?? this, Node);
      while (this.firstChild) this._remove(this.firstChild);
      this._insertMany(nodes, null);
    }

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
      const candidates = node instanceof DocumentFragment ? Array.from(node._esdevChildren()) : [node];
      for (const candidate of candidates) {
        if (candidate instanceof Document || candidate instanceof Attr) {
          throw domError("HierarchyRequestError", "Document and attribute nodes cannot be inserted here.");
        }
      }
      if (this instanceof Document) {
        const elements = Array.from(this._esdevChildren()).filter((node) => node instanceof Element && node !== replacing);
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
      const candidates = node instanceof DocumentFragment ? Array.from(node._esdevChildren()) : [node];
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
        document._adjustRanges?.insert(this, childIndex(candidate));
        this._touch();
        document._queueMutation?.({ type: "childList", target: this, addedNodes: [candidate], removedNodes: [], previousSibling: previous, nextSibling: next });
      }
    }

    _remove(child) {
      child.ownerDocument?._activeElementRemoved?.(child);
      const state = slots(child);
      const parent = slots(this);
      const previousSibling = state.previous;
      const nextSibling = state.next;
      (this.ownerDocument ?? this)._adjustRanges?.remove(this, child, childIndex(child));
      if (state.previous) slots(state.previous).next = state.next; else parent.first = state.next;
      if (state.next) slots(state.next).previous = state.previous; else parent.last = state.previous;
      state.parent = null;
      state.previous = null;
      state.next = null;
      this._touch();
      (this.ownerDocument ?? this)._queueMutation?.({ type: "childList", target: this, addedNodes: [], removedNodes: [child], previousSibling, nextSibling });
    }

    _touch() { slots(this.ownerDocument ?? this).version += 1; }

    get textContent() {
      if (this instanceof Text || this instanceof Comment) return this.data;
      if (this instanceof Attr) return this.value;
      let text = "";
      for (const child of this._esdevChildren()) {
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
        const qualifiedName = this.prefix ? `${this.prefix}:${this.localName}` : this.localName;
        clone = document.createElementNS(this.namespaceURI, qualifiedName);
        for (const attribute of this.attributes) {
          clone.setAttributeNS(attribute.namespaceURI, attribute.name, attribute.value);
        }
      } else if (this instanceof Text) clone = document.createTextNode(this.data);
      else if (this instanceof Comment) clone = document.createComment(this.data);
      else throw domError("NotSupportedError", "This node cannot be cloned.");
      if (deep) for (const child of this._esdevChildren()) clone.appendChild(child.cloneNode(true));
      return clone;
    }
  }

  class CharacterData extends Node {
    constructor(type, name, data, ownerDocument) {
      super(type, name, ownerDocument);
      Object.defineProperty(this, DATA, { value: String(data), writable: true });
    }
    get data() { return this[DATA]; }
    set data(value) {
      const oldValue = this[DATA];
      this[DATA] = String(value);
      this.ownerDocument?._adjustRanges?.characterData(this, oldValue.length, this[DATA].length);
      this.ownerDocument?._queueMutation?.({ type: "characterData", target: this, oldValue });
    }
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
    constructor(name, value, ownerDocument, namespaceURI = null) {
      super(Node.ATTRIBUTE_NODE, name, ownerDocument);
      this.name = name;
      this.value = String(value);
      this.ownerElement = null;
      this.namespaceURI = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
      const separator = name.indexOf(":");
      this.prefix = this.namespaceURI === null || separator === -1 ? null : name.slice(0, separator);
      this.localName = this.namespaceURI === null || separator === -1 ? name : name.slice(separator + 1);
    }
    get nodeValue() { return this.value; }
    set nodeValue(value) { this.value = String(value ?? ""); }
  }

  class NamedNodeMap {
    constructor(element) {
      Object.defineProperty(this, ATTRS, { value: element });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target._list()[Number(property)];
          return Reflect.get(target, property, receiver);
        },
      });
    }
    _list() { return slots(this[ATTRS]).attributes; }
    get length() { return this._list().length; }
    item(index) { return this._list()[index] ?? null; }
    getNamedItem(name) { return this._list().find((attribute) => attribute.name === String(name)) ?? null; }
    getNamedItemNS(namespaceURI, localName) {
      namespaceURI = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
      localName = String(localName);
      return this._list().find((attribute) => attribute.namespaceURI === namespaceURI && attribute.localName === localName) ?? null;
    }
    setNamedItem(attribute) {
      if (!(attribute instanceof Attr)) throw new TypeError("setNamedItem expects an Attr");
      const existing = this.getNamedItem(attribute.name);
      if (attribute.ownerElement && attribute.ownerElement !== this[ATTRS]) throw domError("InUseAttributeError", "The attribute is already in use.");
      if (existing) this._list()[this._list().indexOf(existing)] = attribute; else this._list().push(attribute);
      attribute.ownerElement = this[ATTRS];
      if (existing) existing.ownerElement = null;
      return existing;
    }
    setNamedItemNS(attribute) {
      if (!(attribute instanceof Attr)) throw new TypeError("setNamedItemNS expects an Attr");
      const existing = this.getNamedItemNS(attribute.namespaceURI, attribute.localName);
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
    removeNamedItemNS(namespaceURI, localName) {
      const attribute = this.getNamedItemNS(namespaceURI, localName);
      if (!attribute) throw domError("NotFoundError", "No such attribute.");
      this._list().splice(this._list().indexOf(attribute), 1);
      attribute.ownerElement = null;
      return attribute;
    }
    [Symbol.iterator]() { return this._list()[Symbol.iterator](); }
  }

  class Element extends Node {
    constructor(name, ownerDocument, namespaceURI = HTML_NAMESPACE) {
      const html = namespaceURI === HTML_NAMESPACE;
      super(Node.ELEMENT_NODE, html ? name.toUpperCase() : name, ownerDocument);
      this.namespaceURI = namespaceURI;
      const separator = name.indexOf(":");
      this.prefix = separator === -1 ? null : name.slice(0, separator);
      this.localName = separator === -1 ? name : name.slice(separator + 1);
      this.tagName = html ? name.toUpperCase() : name;
      slots(this).attributes = [];
      slots(this).children = null;
      Object.defineProperty(this, "attributes", { value: new NamedNodeMap(this) });
      Object.defineProperty(this, CLASS_LIST, { value: new DOMTokenList(this) });
      Object.defineProperty(this, DATASET, { value: new DOMStringMap(this) });
    }
    getAttribute(name) { return this.attributes.getNamedItem(String(name))?.value ?? null; }
    getAttributeNode(name) { return this.attributes.getNamedItem(String(name)); }
    hasAttribute(name) { return this.getAttributeNode(name) !== null; }
    getAttributeNS(namespaceURI, localName) { return this.getAttributeNodeNS(namespaceURI, localName)?.value ?? null; }
    getAttributeNodeNS(namespaceURI, localName) { return this.attributes.getNamedItemNS(namespaceURI, localName); }
    hasAttributeNS(namespaceURI, localName) { return this.getAttributeNodeNS(namespaceURI, localName) !== null; }
    setAttribute(name, value) {
      name = String(name);
      const oldValue = this.getAttribute(name);
      this.attributes.setNamedItem(new Attr(name, value, this.ownerDocument)); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: name, oldValue });
    }
    setAttributeNS(namespaceURI, qualifiedName, value) {
      qualifiedName = String(qualifiedName);
      const attribute = new Attr(qualifiedName, value, this.ownerDocument, namespaceURI);
      const oldValue = this.getAttributeNS(attribute.namespaceURI, attribute.localName);
      this.attributes.setNamedItemNS(attribute); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: qualifiedName, oldValue });
    }
    setAttributeNode(attribute) {
      const previous = this.attributes.setNamedItem(attribute); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: previous?.value ?? null });
      return previous;
    }
    removeAttribute(name) {
      const attribute = this.getAttributeNode(name);
      if (attribute) {
        this.attributes.removeNamedItem(name); this._touch();
        this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: attribute.value });
      }
    }
    removeAttributeNS(namespaceURI, localName) {
      const attribute = this.getAttributeNodeNS(namespaceURI, localName);
      if (attribute) {
        this.attributes.removeNamedItemNS(namespaceURI, localName); this._touch();
        this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: attribute.value });
      }
    }
    removeAttributeNode(attribute) {
      if (attribute.ownerElement !== this) throw domError("NotFoundError", "The attribute is not owned by this element.");
      const removed = this.attributes.removeNamedItem(attribute.name); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: removed.name, oldValue: removed.value });
      return removed;
    }
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get classList() { return this[CLASS_LIST]; }
    get dataset() { return this[DATASET]; }
    get children() {
      const state = slots(this);
      return state.children ??= new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((node) => node instanceof Element));
    }
    get firstElementChild() { return this.children.item(0); }
    get lastElementChild() { return this.children.item(this.children.length - 1); }
    get childElementCount() { return this.children.length; }
    getElementsByTagName(name) {
      name = String(name);
      return new HTMLCollection(this, (root) => collect(root, (element) => name === "*" || element.localName === name));
    }
    getElementsByClassName(names) {
      const expected = String(names).trim().split(/\s+/).filter(Boolean);
      return new HTMLCollection(this, (root) => collect(root, (element) => {
        const classes = new Set((element.getAttribute("class") ?? "").trim().split(/\s+/).filter(Boolean));
        return expected.every((name) => classes.has(name));
      }));
    }
  }

  // Reflection is table-driven: the attribute spelling, value kind and
  // coercion live in one place instead of diverging across hand-written
  // accessors. These are the common cross-element attributes; element-specific
  // entries are installed below on the relevant subclass only.
  class HTMLElement extends Element {
    constructor(name, ownerDocument) {
      const context = customConstruction.at(-1);
      if (context && name === undefined && ownerDocument === undefined) {
        super(context.name, context.document);
        return context.element;
      }
      if (name === undefined || ownerDocument === undefined) throw new TypeError("Illegal constructor");
      super(name, ownerDocument);
    }
    click() {
      this.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    }
  }

  // Namespace-specific base classes are observable browser API, even when this
  // layout-free DOM has no SVG or MathML rendering behaviour of its own.
  class SVGElement extends Element {}
  class MathMLElement extends Element {}

  class HTMLInputElement extends HTMLElement {
    constructor(name, ownerDocument) { super(name, ownerDocument); this[INPUT_VALUE] = null; this[INPUT_CHECKED] = null; this[INPUT_INDETERMINATE] = false; }
    click() {
      if (this.disabled) return;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      if (this.type === "checkbox") this.checked = !this.checked;
      if (this.type === "radio" && !this.checked) {
        const name = this.name;
        for (const input of this.ownerDocument.getElementsByTagName("input")) {
          if (input !== this && input.type === "radio" && input.name === name && formOwner(input) === formOwner(this)) input.checked = false;
        }
        this.checked = true;
      }
      if (this.type === "submit") this.form?.requestSubmit(this);
      if (this.type === "reset") this.form?.reset();
    }
  }

  class HTMLButtonElement extends HTMLElement {
    click() {
      if (this.disabled) return;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      if (this.type === "submit") this.form?.requestSubmit(this);
      if (this.type === "reset") this.form?.reset();
    }
  }

  class HTMLDialogElement extends HTMLElement {}

  function isSubmitter(control) {
    return (control instanceof HTMLButtonElement || control instanceof HTMLInputElement) && control.type === "submit";
  }

  function formOwner(control) {
    const id = control.getAttribute("form");
    if (id !== null) return Array.from(control.ownerDocument.getElementsByTagName("form")).find((form) => form.id === id) ?? null;
    for (let parent = control.parentElement; parent; parent = parent.parentElement) if (parent instanceof HTMLFormElement) return parent;
    return null;
  }

  function isDisabled(control) {
    if (control.disabled) return true;
    for (let parent = control.parentElement; parent; parent = parent.parentElement) {
      if (parent instanceof HTMLFieldSetElement || parent instanceof HTMLOptGroupElement) {
        if (parent.disabled) return true;
      }
    }
    return false;
  }

  class HTMLFormElement extends HTMLElement {
    get elements() {
      return new HTMLCollection(this, () => Array.from(this.ownerDocument.getElementsByTagName("*")).filter((element) => ["button", "fieldset", "input", "select", "textarea"].includes(element.localName) && formOwner(element) === this));
    }
    reset() {
      const event = new Event("reset", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      for (const control of this.elements) {
        if (control instanceof HTMLSelectElement) for (const option of control.options) option[SELECTED] = null;
        if (control instanceof HTMLTextAreaElement) control[TEXTAREA_VALUE] = null;
        if (control instanceof HTMLInputElement) { control[INPUT_VALUE] = null; control[INPUT_CHECKED] = null; }
      }
    }
    checkValidity() { return Array.from(this.elements, (control) => control.checkValidity?.() ?? true).every(Boolean); }
    reportValidity() { return this.checkValidity(); }
    requestSubmit(submitter = null) {
      if (submitter !== null && (!isSubmitter(submitter) || submitter.form !== this)) throw new TypeError("requestSubmit submitter must be a submit button belonging to this form");
      submitter ??= Array.from(this.elements).find(isSubmitter) ?? null;
      if (!this.noValidate && !submitter?.formNoValidate && !this.checkValidity()) return;
      this.dispatchEvent(new SubmitEvent("submit", { bubbles: true, cancelable: true, submitter }));
    }
    submit() {}
  }

  class HTMLOptionElement extends HTMLElement {
    constructor(name, ownerDocument) { super(name, ownerDocument); this[SELECTED] = null; }
    get defaultSelected() { return this.hasAttribute("selected"); }
    set defaultSelected(value) { if (value) this.setAttribute("selected", ""); else this.removeAttribute("selected"); }
    get selected() { return this[SELECTED] ?? this.defaultSelected; }
    set selected(value) {
      this[SELECTED] = Boolean(value);
      if (value) {
        for (let parent = this.parentElement; parent; parent = parent.parentElement) {
          if (parent instanceof HTMLSelectElement) {
            if (!parent.multiple) for (const option of parent.options) if (option !== this) option[SELECTED] = false;
            break;
          }
        }
      }
    }
    get value() { return this.getAttribute("value") ?? this.textContent; }
    set value(value) { this.setAttribute("value", String(value)); }
    get index() {
      for (let parent = this.parentElement; parent; parent = parent.parentElement) {
        if (parent instanceof HTMLSelectElement) return Array.from(parent.options).indexOf(this);
      }
      return -1;
    }
  }

  class HTMLSelectElement extends HTMLElement {
    get options() { return new HTMLCollection(this, (root) => collect(root, (element) => element instanceof HTMLOptionElement)); }
    get length() { return this.options.length; }
    set length(value) {
      value = Math.max(0, Math.trunc(Number(value) || 0));
      while (this.options.length > value) this.options.item(this.options.length - 1).remove();
      while (this.options.length < value) this.appendChild(this.ownerDocument.createElement("option"));
    }
    get selectedIndex() {
      const options = Array.from(this.options);
      const selected = options.findIndex((option) => option.selected);
      return selected >= 0 ? selected : !this.multiple && options.length ? 0 : -1;
    }
    set selectedIndex(index) {
      index = Math.trunc(Number(index));
      for (const [at, option] of Array.from(this.options).entries()) option[SELECTED] = at === index;
    }
    get value() { const option = this.options.item(this.selectedIndex); return option?.value ?? ""; }
    set value(value) {
      const option = Array.from(this.options).find((candidate) => candidate.value === String(value));
      if (option) this.selectedIndex = option.index; else this.selectedIndex = -1;
    }
    get selectedOptions() {
      return new HTMLCollection(this, (root) => {
        const options = collect(root, (element) => element instanceof HTMLOptionElement);
        const chosen = options.filter((option) => option.selected);
        return chosen.length || this.multiple ? chosen : options.slice(0, 1);
      });
    }
    add(item, before = null) {
      if (!(item instanceof HTMLOptionElement)) throw new TypeError("select.add expects an option");
      if (typeof before === "number") before = this.options.item(before);
      this.insertBefore(item, before ?? null);
    }
    remove(index) { this.options.item(Number(index))?.remove(); }
  }

  class HTMLTextAreaElement extends HTMLElement {
    constructor(name, ownerDocument) { super(name, ownerDocument); this[TEXTAREA_VALUE] = null; }
    get defaultValue() { return this.textContent; }
    set defaultValue(value) { this.textContent = String(value); if (this[TEXTAREA_VALUE] === null) this[TEXTAREA_VALUE] = null; }
    get value() { return this[TEXTAREA_VALUE] ?? this.defaultValue; }
    set value(value) { this[TEXTAREA_VALUE] = String(value); }
  }

  class HTMLFieldSetElement extends HTMLElement {}
  class HTMLOptGroupElement extends HTMLElement {}

  function validityFor(control) {
    const value = control.value ?? "";
    const required = control.required && (control instanceof HTMLSelectElement ? control.selectedIndex < 0 || value === "" : value === "");
    const typeMismatch = control instanceof HTMLInputElement && value !== "" && (
      control.type === "email" && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value)
      || control.type === "url" && (() => { try { new URL(value); return false; } catch { return true; } })()
    );
    let patternMismatch = false;
    if (control instanceof HTMLInputElement && value !== "" && control.pattern) {
      try { patternMismatch = !(new RegExp(`^(?:${control.pattern})$`, "u")).test(value); } catch {}
    }
    const number = Number(value);
    const rangeUnderflow = control instanceof HTMLInputElement && control.type === "number" && value !== "" && control.min !== "" && Number.isFinite(number) && number < Number(control.min);
    const rangeOverflow = control instanceof HTMLInputElement && control.type === "number" && value !== "" && control.max !== "" && Number.isFinite(number) && number > Number(control.max);
    const tooShort = control.minLength >= 0 && value !== "" && value.length < control.minLength;
    const tooLong = control.maxLength >= 0 && value.length > control.maxLength;
    const customError = Boolean(control[CUSTOM_VALIDITY]);
    return Object.freeze({
      badInput: false, customError, patternMismatch, rangeOverflow, rangeUnderflow,
      stepMismatch: false, tooLong, tooShort, typeMismatch, valid: !(required || typeMismatch || patternMismatch || rangeUnderflow || rangeOverflow || tooShort || tooLong || customError),
      valueMissing: required,
    });
  }

  function installValidation(Class) {
    Object.defineProperties(Class.prototype, {
      willValidate: { get() { return !isDisabled(this); } },
      validity: { get() { return validityFor(this); } },
      validationMessage: { get() { return this.validity.valid ? "" : this[CUSTOM_VALIDITY] || "Constraints not satisfied"; } },
      setCustomValidity: { value(message) { this[CUSTOM_VALIDITY] = String(message); } },
      checkValidity: { value() { if (!this.willValidate || this.validity.valid) return true; this.dispatchEvent(new Event("invalid", { cancelable: true })); return false; } },
      reportValidity: { value() { return this.checkValidity(); } },
    });
  }

  class HTMLLabelElement extends HTMLElement {
    click() {
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      this.control?.click?.();
    }
    get control() {
      if (this.htmlFor) return Array.from(this.ownerDocument.getElementsByTagName("*")).find((element) => element.id === this.htmlFor && isLabelable(element)) ?? null;
      return collect(this, isLabelable)[0] ?? null;
    }
  }

  function isLabelable(element) { return ["button", "input", "select", "textarea"].includes(element.localName); }

  function validDate(value) {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!match) return false;
    const [year, month, day] = match.slice(1).map(Number);
    const date = new Date(Date.UTC(year, month - 1, day));
    return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
  }

  function sanitizeInputValue(type, value) {
    value = String(value);
    if (type === "number") {
      if (!/^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/.test(value)) return "";
      const number = Number(value);
      return Number.isFinite(number) ? String(number) : "";
    }
    if (type === "date") return validDate(value) ? value : "";
    return value;
  }

  function reflectString(attribute) {
    return {
      get() { return this.getAttribute(attribute) ?? ""; },
      set(value) { this.setAttribute(attribute, String(value)); },
    };
  }

  function reflectUrl(attribute) {
    return {
      get() { return new URL(this.getAttribute(attribute) ?? "", globalThis.location?.href ?? "http://localhost/").href; },
      set(value) { this.setAttribute(attribute, String(value)); },
    };
  }

  function reflectBoolean(attribute) {
    return {
      get() { return this.hasAttribute(attribute); },
      set(value) { if (value) this.setAttribute(attribute, ""); else this.removeAttribute(attribute); },
    };
  }

  function reflectInteger(attribute, fallback, minimum = Number.NEGATIVE_INFINITY) {
    return {
      get() {
        const value = Number.parseInt(this.getAttribute(attribute) ?? "", 10);
        return Number.isFinite(value) && value >= minimum ? value : fallback;
      },
      set(value) {
        value = Number(value);
        value = Number.isFinite(value) ? Math.max(minimum, Math.trunc(value)) : fallback;
        this.setAttribute(attribute, String(value));
      },
    };
  }

  function installReflectors(Class, strings = {}, booleans = {}, integers = {}) {
    const properties = {};
    for (const [property, attribute] of Object.entries(strings)) properties[property] = reflectString(attribute);
    for (const [property, attribute] of Object.entries(booleans)) properties[property] = reflectBoolean(attribute);
    for (const [property, [attribute, fallback, minimum]] of Object.entries(integers)) properties[property] = reflectInteger(attribute, fallback, minimum);
    Object.defineProperties(Class.prototype, properties);
  }

  installReflectors(HTMLElement,
    { id: "id", className: "class", title: "title", lang: "lang", dir: "dir", slot: "slot" },
    { hidden: "hidden", inert: "inert" },
    { tabIndex: ["tabindex", -1, Number.NEGATIVE_INFINITY] });
  installReflectors(HTMLInputElement,
    { accept: "accept", alt: "alt", autocomplete: "autocomplete", formEnctype: "formenctype", formMethod: "formmethod", formTarget: "formtarget", name: "name", placeholder: "placeholder" },
    { disabled: "disabled", formNoValidate: "formnovalidate", multiple: "multiple", readOnly: "readonly", required: "required" },
    { maxLength: ["maxlength", -1, -1], minLength: ["minlength", -1, -1], size: ["size", 20, 1] });
  installReflectors(HTMLButtonElement,
    { formEnctype: "formenctype", formMethod: "formmethod", formTarget: "formtarget", name: "name", value: "value" },
    { disabled: "disabled", formNoValidate: "formnovalidate" });
  installReflectors(HTMLDialogElement, {}, { open: "open" });
  installReflectors(HTMLFormElement, { target: "target" }, { noValidate: "novalidate" });
  installReflectors(HTMLLabelElement, { htmlFor: "for" });
  installReflectors(HTMLSelectElement,
    { name: "name" },
    { disabled: "disabled", multiple: "multiple", required: "required" },
    { size: ["size", 0, 0] });
  installReflectors(HTMLTextAreaElement,
    { name: "name", placeholder: "placeholder" },
    { disabled: "disabled", readOnly: "readonly", required: "required" },
    { cols: ["cols", 20, 1], rows: ["rows", 2, 1] });
  installReflectors(HTMLFieldSetElement, { name: "name" }, { disabled: "disabled" });
  installReflectors(HTMLOptGroupElement, { label: "label" }, { disabled: "disabled" });
  installValidation(HTMLInputElement);
  installValidation(HTMLSelectElement);
  installValidation(HTMLTextAreaElement);
  Object.defineProperties(HTMLInputElement.prototype, {
    type: { get() { return this.getAttribute("type") ?? "text"; }, set(value) { this.setAttribute("type", String(value)); } },
    value: {
      get() { return sanitizeInputValue(this.type, this[INPUT_VALUE] ?? this.defaultValue); },
      set(value) { this[INPUT_VALUE] = sanitizeInputValue(this.type, value); },
    },
    defaultValue: {
      get() { return this.getAttribute("value") ?? (["checkbox", "radio"].includes(this.type) ? "on" : ""); },
      set(value) { this.setAttribute("value", String(value)); },
    },
    checked: { get() { return this[INPUT_CHECKED] ?? this.defaultChecked; }, set(value) { this[INPUT_CHECKED] = Boolean(value); } },
    indeterminate: { get() { return this[INPUT_INDETERMINATE]; }, set(value) { this[INPUT_INDETERMINATE] = Boolean(value); } },
    defaultChecked: { get() { return this.hasAttribute("checked"); }, set(value) { if (value) this.setAttribute("checked", ""); else this.removeAttribute("checked"); } },
    min: { get() { return this.getAttribute("min") ?? ""; }, set(value) { this.setAttribute("min", String(value)); } },
    max: { get() { return this.getAttribute("max") ?? ""; }, set(value) { this.setAttribute("max", String(value)); } },
    pattern: { get() { return this.getAttribute("pattern") ?? ""; }, set(value) { this.setAttribute("pattern", String(value)); } },
    valueAsNumber: {
      get() {
        if (this.type === "number") return this.value === "" ? NaN : Number(this.value);
        if (this.type === "date") return this.value === "" ? NaN : Date.parse(`${this.value}T00:00:00.000Z`);
        return NaN;
      },
      set(value) {
        if (!(["number", "date"].includes(this.type))) throw new DOMException("This input type has no numeric value.", "InvalidStateError");
        if (Number.isNaN(Number(value))) { this.value = ""; return; }
        if (this.type === "number") { this.value = Number(value); return; }
        this.valueAsDate = new Date(Number(value));
      },
    },
    valueAsDate: {
      get() { return this.type === "date" && this.value !== "" ? new Date(`${this.value}T00:00:00.000Z`) : null; },
      set(value) {
        if (this.type !== "date") throw new DOMException("This input type has no date value.", "InvalidStateError");
        if (value === null) { this.value = ""; return; }
        if (!(value instanceof Date) || Number.isNaN(value.getTime())) throw new TypeError("valueAsDate expects a valid Date");
        this.value = `${value.getUTCFullYear().toString().padStart(4, "0")}-${(value.getUTCMonth() + 1).toString().padStart(2, "0")}-${value.getUTCDate().toString().padStart(2, "0")}`;
      },
    },
  });
  Object.defineProperties(HTMLButtonElement.prototype, {
    type: { get() { return this.getAttribute("type") ?? "submit"; }, set(value) { this.setAttribute("type", String(value)); } },
  });
  Object.defineProperties(HTMLFormElement.prototype, {
    action: reflectUrl("action"),
    method: { get() { return (this.getAttribute("method") ?? "get").toLowerCase(); }, set(value) { this.setAttribute("method", String(value).toLowerCase()); } },
    enctype: { get() { return this.getAttribute("enctype") ?? "application/x-www-form-urlencoded"; }, set(value) { this.setAttribute("enctype", String(value)); } },
  });
  Object.defineProperties(HTMLButtonElement.prototype, { formAction: reflectUrl("formaction") });
  Object.defineProperties(HTMLInputElement.prototype, { formAction: reflectUrl("formaction") });
  for (const Class of [HTMLInputElement, HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement]) {
    Object.defineProperty(Class.prototype, "form", { get() { return formOwner(this); } });
    Object.defineProperty(Class.prototype, "labels", {
      get() {
        return new HTMLCollection(this.ownerDocument, (root) => collect(root, (element) => element instanceof HTMLLabelElement && element.control === this));
      },
    });
  }

  class DocumentFragment extends Node {
    constructor(ownerDocument) { super(Node.DOCUMENT_FRAGMENT_NODE, "#document-fragment", ownerDocument); }
  }

  class ShadowRoot extends DocumentFragment {
    constructor(host, mode) {
      super(host.ownerDocument);
      this.host = host;
      this.mode = mode;
      this.delegatesFocus = false;
    }
    _eventParent(event) { return event.composed ? this.host : null; }
  }

  Object.defineProperties(Element.prototype, {
    attachShadow: { value(options = {}) {
      if (this[SHADOW_ROOT]) throw domError("NotSupportedError", "This element already hosts a shadow root.");
      const mode = options.mode;
      if (mode !== "open" && mode !== "closed") throw new TypeError("attachShadow requires mode 'open' or 'closed'.");
      const root = new ShadowRoot(this, mode);
      this[SHADOW_ROOT] = root;
      return root;
    } },
    shadowRoot: { get() { const root = this[SHADOW_ROOT]; return root?.mode === "open" ? root : null; } },
  });

  class Document extends Node {
    constructor() {
      super(Node.DOCUMENT_NODE, "#document", null);
      slots(this).ownerDocument = this;
      slots(this).version = 0;
    }
    get documentElement() { return Array.from(this._esdevChildren()).find((node) => node instanceof Element) ?? null; }
    createElement(name) {
      name = String(name);
      if (!/^[a-z][a-z0-9_:-]*$/.test(name)) throw domError("InvalidCharacterError", "Element names must be lowercase modern HTML names.");
      return new (ELEMENT_CLASSES[name] ?? HTMLElement)(name, this);
    }
    createElementNS(namespaceURI, qualifiedName) {
      namespaceURI = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
      qualifiedName = String(qualifiedName);
      if (!/^[A-Za-z][A-Za-z0-9_:-]*$/.test(qualifiedName)) throw domError("InvalidCharacterError", "Element names must be valid XML qualified names.");
      if (namespaceURI === HTML_NAMESPACE) return new (ELEMENT_CLASSES[qualifiedName] ?? HTMLElement)(qualifiedName, this);
      if (namespaceURI === SVG_NAMESPACE) return new SVGElement(qualifiedName, this, namespaceURI);
      if (namespaceURI === MATHML_NAMESPACE) return new MathMLElement(qualifiedName, this, namespaceURI);
      return new Element(qualifiedName, this, namespaceURI);
    }
    createTextNode(data) { return new Text(data, this); }
    createComment(data) { return new Comment(data, this); }
    createDocumentFragment() { return new DocumentFragment(this); }
    createAttribute(name) { return new Attr(String(name), "", this); }
    createAttributeNS(namespaceURI, qualifiedName) { return new Attr(String(qualifiedName), "", this, namespaceURI); }
    getElementsByTagName(name) {
      name = String(name);
      return new HTMLCollection(this, (root) => collect(root, (element) => name === "*" || element.localName === name));
    }
    getElementsByClassName(names) {
      const expected = String(names).trim().split(/\s+/).filter(Boolean);
      return new HTMLCollection(this, (root) => collect(root, (element) => {
        const classes = new Set((element.getAttribute("class") ?? "").trim().split(/\s+/).filter(Boolean));
        return expected.every((name) => classes.has(name));
      }));
    }
    getElementById(id) {
      id = String(id);
      return collect(this, (element) => element.id === id)[0] ?? null;
    }
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

  const ELEMENT_CLASSES = {
    button: HTMLButtonElement,
    dialog: HTMLDialogElement,
    form: HTMLFormElement,
    input: HTMLInputElement,
    label: HTMLLabelElement,
    option: HTMLOptionElement,
    optgroup: HTMLOptGroupElement,
    select: HTMLSelectElement,
    textarea: HTMLTextAreaElement,
    fieldset: HTMLFieldSetElement,
  };

  function upgradeCustom(element, constructor) {
    if (Object.getPrototypeOf(element) === constructor.prototype) return element;
    if (!(constructor.prototype instanceof HTMLElement)) throw new TypeError("Custom element constructors must extend HTMLElement");
    Object.setPrototypeOf(element, constructor.prototype);
    customConstruction.push({ element, name: element.localName, document: element.ownerDocument });
    try {
      const constructed = new constructor();
      if (constructed !== element) throw new TypeError("Custom element constructor returned a different object");
    } finally {
      customConstruction.pop();
    }
    return element;
  }

  function collect(root, predicate) {
    const result = [];
    for (const child of root._esdevChildren()) {
      if (child instanceof Element) {
        if (predicate(child)) result.push(child);
        result.push(...collect(child, predicate));
      }
    }
    return result;
  }

  return { Node, NodeList, HTMLCollection, DOMTokenList, Document, DocumentFragment, ShadowRoot, Element, HTMLElement, SVGElement, MathMLElement, HTMLInputElement, HTMLButtonElement, HTMLDialogElement, HTMLFormElement, HTMLLabelElement, HTMLFieldSetElement, HTMLOptGroupElement, HTMLOptionElement, HTMLSelectElement, HTMLTextAreaElement, Text, Comment, Attr, NamedNodeMap, VOID, HTML_NAMESPACE, SVG_NAMESPACE, MATHML_NAMESPACE, isDisabled, upgradeCustom };
}
