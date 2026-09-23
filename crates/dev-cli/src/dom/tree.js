// The mutable core of esdev's test-only DOM. It is a factory rather than a
// global installer so the test runner can create one isolated realm per file.
// Public classes keep state in symbols: framework code inspecting an element
// sees the DOM surface, not the linked-list bookkeeping behind it.

const SLOT = Symbol("esdev DOM slots");
const ATTRS = Symbol("esdev DOM attributes");
const NAMED_ATTRIBUTES = Symbol("esdev DOM named attributes");
const CLASS_LIST = Symbol("esdev DOM class list");
const DATASET = Symbol("esdev DOM dataset");
const DATA = Symbol("esdev DOM character data");
const SELECTED = Symbol("esdev DOM option selected state");
const TEMPLATE_CONTENT = Symbol("esdev DOM template content");
const VALIDITY = Symbol("esdev DOM validity state");
const VALIDITY_CONTROL = Symbol("esdev DOM validity control");
const IMPLEMENTATION = Symbol("esdev DOM implementation");
const ITERATOR = Symbol("esdev DOM node iterator position");
const SHADOW_OPTIONS = Symbol("esdev DOM shadow root options");
const INTERNALS = Symbol("esdev DOM element internals");
const FORM_VALUE = Symbol("esdev DOM submission value");
const FORM_VALIDITY = Symbol("esdev DOM internals validity");
const STATES = Symbol("esdev DOM custom state set");
const FORM_STATE = Symbol("esdev DOM submission state");
const MANUAL_ASSIGNED = Symbol("esdev DOM manually assigned nodes");
const SLOT_PENDING = Symbol("esdev DOM pending slotchange");
const FORM_DISABLED = Symbol("esdev DOM last reported disabled state");
const INERT = Symbol("esdev DOM inert subtree");
const DIALOG_MODAL = Symbol("esdev DOM dialog modality");
const DIALOG_RETURN = Symbol("esdev DOM dialog return value");
const POPOVER_OPEN = Symbol("esdev DOM popover state");
const TEXTAREA_VALUE = Symbol("esdev DOM textarea value state");
const CUSTOM_VALIDITY = Symbol("esdev DOM custom validity");
const INPUT_VALUE = Symbol("esdev DOM input value state");
const INPUT_CHECKED = Symbol("esdev DOM input checked state");
const INPUT_INDETERMINATE = Symbol("esdev DOM input indeterminate state");
const INPUT_SELECTION_START = Symbol("esdev DOM input selection start");
const INPUT_SELECTION_END = Symbol("esdev DOM input selection end");
const INPUT_SELECTION_DIRECTION = Symbol("esdev DOM input selection direction");
const SHADOW_ROOT = Symbol("esdev DOM shadow root");
const COLLECTION = Symbol("esdev DOM live collection state");
const HTML_NAMESPACE = "http://www.w3.org/1999/xhtml";
const SVG_NAMESPACE = "http://www.w3.org/2000/svg";
const MATHML_NAMESPACE = "http://www.w3.org/1998/Math/MathML";
// The elements a shadow root may be attached to (HTML, "valid shadow host name").
const SHADOW_HOSTS = new Set([
  "article", "aside", "blockquote", "body", "div", "footer", "h1", "h2", "h3", "h4", "h5", "h6",
  "header", "main", "nav", "p", "section", "span",
]);
// Every element the modern HTML standard names. Legacy ones a browser still
// answers for — `marquee`, `frameset`, `xmp` — are deliberately absent: this
// DOM is the modern language, and `{ extends: "marquee" }` is not something to
// help a project do.
const HTML_ELEMENT_NAMES = new Set([
  "html", "head", "title", "base", "link", "meta", "style", "body", "article", "section",
  "nav", "aside", "h1", "h2", "h3", "h4", "h5", "h6", "hgroup", "header", "footer", "address",
  "p", "hr", "pre", "blockquote", "ol", "ul", "menu", "li", "dl", "dt", "dd", "figure",
  "figcaption", "main", "search", "div", "a", "em", "strong", "small", "s", "cite", "q", "dfn",
  "abbr", "ruby", "rt", "rp", "data", "time", "code", "var", "samp", "kbd", "sub", "sup", "i",
  "b", "u", "mark", "bdi", "bdo", "span", "br", "wbr", "ins", "del", "picture", "source",
  "img", "iframe", "embed", "object", "video", "audio", "track", "map", "area", "table",
  "caption", "colgroup", "col", "tbody", "thead", "tfoot", "tr", "td", "th", "form", "label",
  "input", "button", "select", "datalist", "optgroup", "option", "textarea", "output",
  "progress", "meter", "fieldset", "legend", "details", "summary", "dialog", "script",
  "noscript", "template", "slot", "canvas"
]);

const VOID = new Set(["area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "source", "track", "wbr"]);

function domError(name, message) {
  return new DOMException(message, name);
}

// ASCII case changes, which is what the DOM specifies for names: `toLowerCase`
// would also fold `Ä` to `ä`, and a browser does not.
const asciiLowercase = (text) => text.replace(/[A-Z]+/g, (letters) => letters.toLowerCase());
const asciiUppercase = (text) => text.replace(/[a-z]+/g, (letters) => letters.toUpperCase());

// A document is an HTML document or an XML one, and names behave differently in
// each: `new Document()` and `createDocument()` make XML documents, where
// `createElement("DIV")` keeps its case.
function isHTMLDocument(document) {
  return document != null && slots(document)?.html === true;
}

// An attribute name as this element stores it. HTML elements in an HTML
// document lowercase it — `setAttribute("tabIndex", 0)` sets `tabindex` — and
// anything else does not, because in SVG and XML the case is the name.
function qualified(element, name) {
  name = String(name);
  return element.namespaceURI === HTML_NAMESPACE && isHTMLDocument(element.ownerDocument) ? asciiLowercase(name) : name;
}

// The DOM's name rules, as relaxed in 2025 and shipped by Chrome: a name is
// refused for the few characters that would break markup, not for failing the
// XML `Name` production.
const XML_NAMESPACE = "http://www.w3.org/XML/1998/namespace";
const XMLNS_NAMESPACE = "http://www.w3.org/2000/xmlns/";
function isValidElementLocalName(name) {
  if (name.length === 0) return false;
  if (/^[A-Za-z]/.test(name)) return !/[\t\n\f\r \0/>]/.test(name);
  const [first, ...rest] = name;
  if (!(first === ":" || first === "_" || first.codePointAt(0) >= 0x80)) return false;
  return rest.every((character) => /[A-Za-z0-9\-.:_]/.test(character) || character.codePointAt(0) >= 0x80);
}
const isValidAttributeLocalName = (name) => name.length > 0 && !/[\t\n\f\r \0/=>]/.test(name);
const isValidNamespacePrefix = (name) => name.length > 0 && !/[\t\n\f\r \0/>]/.test(name);
const isValidDoctypeName = (name) => !/[\t\n\f\r \0>]/.test(name);

// "Validate and extract": a namespace and a qualified name become a namespace,
// prefix and local name, or an error. The split is the specification's strict
// one, so `a:b:c` is prefix `a` and local name `b`.
function validateAndExtract(namespace, qualifiedName, context) {
  namespace = namespace == null || namespace === "" ? null : String(namespace);
  qualifiedName = String(qualifiedName);
  let prefix = null;
  let localName = qualifiedName;
  if (qualifiedName.includes(":")) {
    const parts = qualifiedName.split(":");
    prefix = parts[0];
    localName = parts[1];
    if (!isValidNamespacePrefix(prefix)) throw new DOMException(`"${prefix}" is not a valid namespace prefix.`, "InvalidCharacterError");
  }
  const valid = context === "attribute" ? isValidAttributeLocalName(localName) : isValidElementLocalName(localName);
  if (!valid) throw new DOMException(`"${qualifiedName}" is not a valid ${context} name.`, "InvalidCharacterError");
  if (prefix !== null && namespace === null) throw new DOMException("A prefixed name needs a namespace.", "NamespaceError");
  if (prefix === "xml" && namespace !== XML_NAMESPACE) throw new DOMException("The xml prefix is reserved for the XML namespace.", "NamespaceError");
  if ((qualifiedName === "xmlns" || prefix === "xmlns") !== (namespace === XMLNS_NAMESPACE)) {
    throw new DOMException("xmlns names belong to the XMLNS namespace, and only they do.", "NamespaceError");
  }
  return { namespace, prefix, localName };
}

// The element's own attribute map, read from its slot rather than through the
// public `attributes` getter. A test that spies on `Element.prototype.attributes`
// — to assert that rendering does not read the DOM — must not see every internal
// attribute operation go through it; a browser's internals do not either.
function ownAttributes(element) {
  return element[NAMED_ATTRIBUTES];
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

// Web IDL puts an interface's members on the prototype as **configurable**, and
// a test relies on it: `vi.spyOn(input, "checked", "set")` and every other stub
// redefines the property it is replacing, and a descriptor that forgot the flag
// answers `Cannot redefine property`. Enumerable too, as a browser has them.
// A symbol-keyed slot is this DOM's own bookkeeping and stays hidden and fixed.
function defineIdl(target, properties) {
  const described = {};
  for (const name of Reflect.ownKeys(properties)) {
    const descriptor = properties[name];
    if (typeof name === "symbol") {
      described[name] = descriptor;
      continue;
    }
    // An operation is writable as well as configurable — Web IDL says so, and a
    // test that replaces a method (`el.focus = spy`) needs it.
    const writable = typeof descriptor.value === "function" ? { writable: true } : {};
    described[name] = { configurable: true, enumerable: true, ...writable, ...descriptor };
  }
  Object.defineProperties(target, described);
  return target;
}

export function createTree(events = {}) {
  const { EventTarget = class {}, Event = class {}, MouseEvent = class {}, SubmitEvent = class {}, CommandEvent = class {}, ToggleEvent = Event } = events;
  const customConstruction = [];
  // Set by the custom-element registry, which is the only thing that knows
  // which class was defined under which name.
  let customLookup = null;
  const NodeFilter = Object.freeze({
    FILTER_ACCEPT: 1, FILTER_REJECT: 2, FILTER_SKIP: 3,
    SHOW_ALL: 0xFFFFFFFF, SHOW_ELEMENT: 0x1, SHOW_TEXT: 0x4, SHOW_COMMENT: 0x80,
  });
  class LiveCollection {
    constructor(root, filter) {
      Object.defineProperty(this, COLLECTION, { value: { root, filter, version: -1, values: [] } });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target._values()[Number(property)];
          if (typeof property === "string") {
            const named = target._namedProperty(property);
            if (named !== undefined) return named;
          }
          return Reflect.get(target, property, receiver);
        },
        has(target, property) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return Number(property) < target._values().length;
          if (typeof property === "string" && target._namedProperty(property) !== undefined) return true;
          return Reflect.has(target, property);
        },
        ownKeys(target) {
          return [...target._values().keys()].map(String).concat(target._namedProperties(), Reflect.ownKeys(target));
        },
        getOwnPropertyDescriptor(target, property) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) {
            const value = target._values()[Number(property)];
            return value === undefined ? undefined : { configurable: true, enumerable: true, value, writable: false };
          }
          if (typeof property === "string") {
            const value = target._namedProperty(property);
            if (value !== undefined) return { configurable: true, enumerable: false, value, writable: false };
          }
          return Reflect.getOwnPropertyDescriptor(target, property);
        },
      });
    }
    _values() {
      const state = this[COLLECTION];
      // A static list — `querySelectorAll`, a mutation record — is a snapshot.
      if (state.root === null) return state.values;
      const document = state.root.ownerDocument ?? state.root;
      const version = slots(document).version;
      if (state.version !== version) {
        state.values = state.filter(state.root);
        state.version = version;
      }
      return state.values;
    }
    _namedProperties() { return []; }
    _namedProperty(name) { return undefined; }
    get length() { return this._values().length; }
    item(index) { return this._values()[index] ?? null; }
  }
  // Every list with an indexed getter iterates as an array does — Web IDL makes
  // `@@iterator` Array's own `values`, not a copy of it.
  LiveCollection.prototype[Symbol.iterator] = Array.prototype.values;

  class NodeList extends LiveCollection {}
  // `iterable<Node>`: the whole set, and each one Array's.
  iterableLike(NodeList);

  function iterableLike(List) {
    for (const name of ["entries", "keys", "values", "forEach"]) {
      Object.defineProperty(List.prototype, name, { value: Array.prototype[name], writable: true, enumerable: true, configurable: true });
    }
    Object.defineProperty(List.prototype, Symbol.iterator, { value: Array.prototype.values, writable: true, configurable: true });
  }

  function staticNodeList(values) {
    const list = new NodeList(null, null);
    list[COLLECTION].values = [...values];
    return list;
  }
  class HTMLCollection extends LiveCollection {
    _namedProperties() {
      const values = this._values();
      const names = [];
      const seen = new Set();
      for (const attribute of ["id", "name"]) {
        for (const element of values) {
          const name = element.getAttribute(attribute);
          if (name && !/^(0|[1-9][0-9]*)$/.test(name) && !seen.has(name)) { seen.add(name); names.push(name); }
        }
      }
      return names;
    }
    _namedProperty(name) {
      return this._values().find((element) => element.id === name || element.getAttribute("name") === name);
    }
    namedItem(name) {
      name = String(name);
      return this._namedProperty(name) ?? null;
    }
  }

  // The DOM's ordered set of tokens over one attribute: `classList`, and the
  // `relList`, `htmlFor`, `sandbox` and `sizes` of the elements that have them.
  // Its tokens are split on ASCII white space only — a no-break space is part
  // of a token — and an edit that leaves the set empty on an element without
  // the attribute does not create one.
  const ASCII_WHITESPACE = /[\t\n\f\r ]/;
  class DOMTokenList {
    constructor(element, attribute = "class", supported = null) {
      Object.defineProperty(this, ATTRS, { value: { element, attribute, supported } });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target._tokens()[Number(property)];
          return Reflect.get(target, property, receiver);
        },
        has(target, property) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return Number(property) < target._tokens().length;
          return Reflect.has(target, property);
        },
      });
    }
    _tokens() {
      const { element, attribute } = this[ATTRS];
      const tokens = [];
      for (const token of (element.getAttribute(attribute) ?? "").split(/[\t\n\f\r ]+/)) {
        if (token && !tokens.includes(token)) tokens.push(token);
      }
      return tokens;
    }
    _update(tokens) {
      const { element, attribute } = this[ATTRS];
      if (!element.hasAttribute(attribute) && tokens.length === 0) return;
      element.setAttribute(attribute, tokens.join(" "));
    }
    _validate(token) {
      token = String(token);
      if (token === "") throw domError("SyntaxError", "The token must not be empty.");
      if (ASCII_WHITESPACE.test(token)) throw domError("InvalidCharacterError", "The token must not contain ASCII whitespace.");
      return token;
    }
    get length() { return this._tokens().length; }
    get value() { const { element, attribute } = this[ATTRS]; return element.getAttribute(attribute) ?? ""; }
    set value(value) { const { element, attribute } = this[ATTRS]; element.setAttribute(attribute, String(value)); }
    toString() { return this.value; }
    item(index) { return this._tokens()[toUnsignedLong(index)] ?? null; }
    contains(token) { return this._tokens().includes(String(token)); }
    add(...tokens) {
      tokens = tokens.map((token) => this._validate(token));
      const next = this._tokens();
      for (const token of tokens) if (!next.includes(token)) next.push(token);
      this._update(next);
    }
    remove(...tokens) {
      tokens = new Set(tokens.map((token) => this._validate(token)));
      this._update(this._tokens().filter((token) => !tokens.has(token)));
    }
    toggle(token, force) {
      token = this._validate(token);
      const tokens = this._tokens();
      if (tokens.includes(token)) {
        if (force === undefined || !force) {
          this._update(tokens.filter((each) => each !== token));
          return false;
        }
        return true;
      }
      if (force === undefined || force) {
        tokens.push(token);
        this._update(tokens);
        return true;
      }
      return false;
    }
    replace(token, replacement) {
      token = String(token);
      replacement = String(replacement);
      if (token === "" || replacement === "") throw domError("SyntaxError", "The token must not be empty.");
      if (ASCII_WHITESPACE.test(token) || ASCII_WHITESPACE.test(replacement)) {
        throw domError("InvalidCharacterError", "The token must not contain ASCII whitespace.");
      }
      const tokens = this._tokens();
      if (!tokens.includes(token)) return false;
      // The ordered set's replace: the first of either becomes the
      // replacement, and every other instance of either goes.
      const first = tokens.findIndex((each) => each === token || each === replacement);
      this._update(tokens.flatMap((each, index) =>
        index === first ? [replacement] : each === token || each === replacement ? [] : [each]));
      return true;
    }
    supports(token) {
      const { supported, attribute } = this[ATTRS];
      if (!supported) throw new TypeError(`The ${attribute} attribute has no supported tokens.`);
      return supported.has(String(token).toLowerCase());
    }
  }
  iterableLike(DOMTokenList);

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
            ...Array.from(ownAttributes(target[ATTRS]), (attribute) => datasetProperty(attribute.name)).filter((property) => property !== null),
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
    static CDATA_SECTION_NODE = 4;
    static PROCESSING_INSTRUCTION_NODE = 7;
    static DOCUMENT_TYPE_NODE = 10;

    static DOCUMENT_POSITION_DISCONNECTED = 1;
    static DOCUMENT_POSITION_PRECEDING = 2;
    static DOCUMENT_POSITION_FOLLOWING = 4;
    static DOCUMENT_POSITION_CONTAINS = 8;
    static DOCUMENT_POSITION_CONTAINED_BY = 16;
    static DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC = 32;

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
    contains(other) { return other instanceof Node && isInclusiveAncestor(this, other); }
    hasAttributes() { return this instanceof Element && ownAttributes(this).length !== 0; }

    appendChild(node) {
      if (!(node instanceof Node)) throw new TypeError("appendChild expects a Node");
      this._preInsert(node, null);
      return node;
    }

    // A move that keeps state: no removing and re-inserting, so nothing is
    // disconnected and reconnected, no custom element reaction runs, and
    // whatever an element was holding — a control's value, a `<details>` being
    // open — it still holds. Both nodes must already be in this tree.
    moveBefore(node, child) {
      if (!(node instanceof Node)) throw new TypeError("moveBefore expects a Node");
      if (child !== null && child !== undefined && !(child instanceof Node)) {
        throw new TypeError("moveBefore expects a Node or null as the reference child");
      }
      const reference = child ?? null;
      if (isInclusiveAncestor(node, this)) {
        throw domError("HierarchyRequestError", "A node cannot be moved into one of its descendants.");
      }
      if (reference !== null && reference.parentNode !== this) {
        throw domError("NotFoundError", "The reference child is not a child of this node.");
      }
      // "Already in this tree" is the whole point: a move that would connect or
      // disconnect the node is an insertion, and `insertBefore` is that.
      if (node.parentNode === null || node.getRootNode() !== this.getRootNode()) {
        throw domError("HierarchyRequestError", "moveBefore only moves a node that is already in this tree.");
      }
      this._validateInsertion(node, reference, null);
      if (node === reference) return node;
      const from = node.parentNode;
      const state = slots(node);
      const previous = state.previous;
      const next = state.next;
      const index = childIndex(node);
      // The pointers are moved here rather than through insert and remove,
      // because those are where the reactions live and a move runs none.
      const leaving = slots(from);
      if (previous) slots(previous).next = next; else leaving.first = next;
      if (next) slots(next).previous = previous; else leaving.last = previous;
      (from.ownerDocument ?? from)._adjustRanges?.remove(from, node, index);
      (from.ownerDocument ?? from)._queueMutation?.({ type: "childList", target: from, addedNodes: [], removedNodes: [node], previousSibling: previous, nextSibling: next });
      const arriving = slots(this);
      const before = reference;
      const after = before ? slots(before).previous : arriving.last;
      state.parent = this;
      state.previous = after;
      state.next = before;
      if (after) slots(after).next = node; else arriving.first = node;
      if (before) slots(before).previous = node; else arriving.last = node;
      (this.ownerDocument ?? this)._adjustRanges?.insert(this, childIndex(node));
      (this.ownerDocument ?? this)._queueMutation?.({ type: "childList", target: this, addedNodes: [node], removedNodes: [], previousSibling: after, nextSibling: before });
      this._touch();
      from._touch();
      signalSlotChange(from instanceof Element ? from : null);
      signalSlotChange(this instanceof Element ? this : null);
      return node;
    }

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
      // Through the internal primitive, not `removeChild`: a browser's
      // `remove()` is one observable operation, and anything watching
      // `removeChild` — a spy, a patch, an override — must not see a second.
      if (this.parentNode) this.parentNode._remove(this);
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
      parent._remove(this);
    }

    append(...items) { this._insertMany(asNodes(items, this.ownerDocument ?? this, Node), null); }
    prepend(...items) { this._insertMany(asNodes(items, this.ownerDocument ?? this, Node), this.firstChild); }
    replaceChildren(...items) {
      const nodes = asNodes(items, this.ownerDocument ?? this, Node);
      while (this.firstChild) this._remove(this.firstChild);
      this._insertMany(nodes, null);
    }

    // The spec's "replace all", as one operation rather than as a call to
    // `replaceChildren`: `innerHTML` is defined in terms of this, and a browser
    // makes no public call a patched prototype could see.
    _replaceAll(node) {
      while (this.firstChild) this._remove(this.firstChild);
      if (node) this._insert(node, null);
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
        const doctypes = Array.from(this._esdevChildren()).filter((node) => node instanceof DocumentType && node !== replacing);
        const incomingDoctypes = candidates.filter((node) => node instanceof DocumentType);
        if (doctypes.length + incomingDoctypes.length > 1) {
          throw domError("HierarchyRequestError", "A document can have only one doctype.");
        }
        if (incomingDoctypes.length > 0 && elements.length > 0 && (before === null || childIndex(before) > childIndex(elements[0]))) {
          throw domError("HierarchyRequestError", "A doctype must precede the document element.");
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
        signalSlotChange(this instanceof Element ? this : null);
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
      (this.ownerDocument ?? this)._keepObserving?.(this, child);
      (this.ownerDocument ?? this)._queueMutation?.({ type: "childList", target: this, addedNodes: [], removedNodes: [child], previousSibling, nextSibling });
      signalSlotChange(this instanceof Element ? this : null);
    }

    _touch() { slots(this.ownerDocument ?? this).version += 1; }

    get textContent() {
      if (this instanceof Text || this instanceof Comment) return this.data;
      if (this instanceof Attr) return this.value;
      // A document and a doctype have no text content — not an empty string,
      // `null` — and assigning to one does nothing. Anything else would let
      // `document.textContent = ""` empty the document.
      if (this instanceof Document || this instanceof DocumentType) return null;
      let text = "";
      for (const child of this._esdevChildren()) {
        if (!(child instanceof Comment)) text += child.textContent;
      }
      return text;
    }

    set textContent(value) {
      if (this instanceof Text || this instanceof Comment) { this.data = value ?? ""; return; }
      if (this instanceof Attr) { this.value = value ?? ""; return; }
      if (this instanceof Document || this instanceof DocumentType) return;
      while (this.firstChild) this._remove(this.firstChild);
      if (value !== null && value !== "") this._insert((this.ownerDocument ?? this).createTextNode(String(value)), null);
    }

    isSameNode(other) { return other === this; }

    // Equality is by kind, name and children, never by identity: two separately
    // created trees with the same shape are equal nodes.
    // The document's base URL: its first `<base href>`, resolved against the
    // document's URL, or that URL. A document with no URL of its own — made by
    // `createDocument`, `createHTMLDocument` or `new Document()` — falls back
    // to the base URL of the document that made it, as in a browser.
    get baseURI() {
      const document = this.ownerDocument ?? this;
      const fallback = document.URL === "about:blank" && currentDocument && currentDocument !== document
        ? currentDocument.baseURI
        : document.URL;
      const base = collect(document, (element) => element.localName === "base" && element.namespaceURI === HTML_NAMESPACE && element.hasAttribute("href"))[0];
      if (!base) return fallback;
      try {
        return new URL(base.getAttribute("href"), fallback).href;
      } catch {
        return fallback;
      }
    }
    isEqualNode(other) {
      // By type, then by what the specification lists for that type — not by
      // `nodeName`, which for an element depends on its document's kind.
      if (!(other instanceof Node) || other.nodeType !== this.nodeType) return false;
      if (this instanceof DocumentType && (other.name !== this.name || other.publicId !== this.publicId || other.systemId !== this.systemId)) return false;
      if (this instanceof Element) {
        if (other.namespaceURI !== this.namespaceURI || other.prefix !== this.prefix || other.localName !== this.localName) return false;
        if (ownAttributes(other).length !== ownAttributes(this).length) return false;
        for (const attribute of ownAttributes(this)) {
          const match = other.getAttributeNS(attribute.namespaceURI, attribute.localName);
          if (match === null || match !== attribute.value) return false;
        }
      }
      if (this instanceof Attr && (other.namespaceURI !== this.namespaceURI || other.localName !== this.localName || other.value !== this.value)) return false;
      if (this instanceof CharacterData && other.data !== this.data) return false;
      if (this instanceof ProcessingInstruction && other.target !== this.target) return false;
      const ours = Array.from(this._esdevChildren());
      const theirs = Array.from(other._esdevChildren?.() ?? []);
      return ours.length === theirs.length && ours.every((child, index) => child.isEqualNode(theirs[index]));
    }

    // Tree order, as the specification defines it: the comparison is made
    // against the common inclusive ancestor, so a node reports its own
    // descendants as CONTAINED_BY and FOLLOWING, not merely as later.
    compareDocumentPosition(other) {
      if (!(other instanceof Node)) throw new TypeError("compareDocumentPosition expects a Node");
      if (other === this) return 0;
      const ancestry = (node) => { const chain = []; for (let step = node; step; step = step.parentNode ?? step.host ?? null) chain.unshift(step); return chain; };
      const ours = ancestry(this);
      const theirs = ancestry(other);
      if (ours[0] !== theirs[0]) {
        // Disconnected trees still order consistently for a given pair, which
        // is all the specification asks of an implementation-specific answer.
        return Node.DOCUMENT_POSITION_DISCONNECTED | Node.DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC
          | Node.DOCUMENT_POSITION_PRECEDING;
      }
      let depth = 0;
      while (ours[depth] === theirs[depth] && depth < ours.length && depth < theirs.length) depth += 1;
      if (depth === ours.length) return Node.DOCUMENT_POSITION_CONTAINED_BY | Node.DOCUMENT_POSITION_FOLLOWING;
      if (depth === theirs.length) return Node.DOCUMENT_POSITION_CONTAINS | Node.DOCUMENT_POSITION_PRECEDING;
      const siblings = Array.from(ours[depth - 1]._esdevChildren());
      return siblings.indexOf(ours[depth]) < siblings.indexOf(theirs[depth])
        ? Node.DOCUMENT_POSITION_FOLLOWING
        : Node.DOCUMENT_POSITION_PRECEDING;
    }

    // Contiguous text nodes become one and empty ones go, which is what a
    // framework's diffing assumptions and `wholeText` both rely on.
    normalize() {
      for (const child of Array.from(this._esdevChildren())) {
        // The list was snapshotted, so a child merged into an earlier one is
        // already gone by the time the loop reaches it.
        if (child.parentNode !== this) continue;
        if (!(child instanceof Text)) { child.normalize(); continue; }
        if (child.data.length === 0) { this._remove(child); continue; }
        let next = child.nextSibling;
        while (next instanceof Text) {
          const following = next.nextSibling;
          child.data += next.data;
          this._remove(next);
          next = following;
        }
      }
    }

    cloneNode(deep = false) {
      const document = this.ownerDocument ?? this;
      let clone;
      if (this instanceof Document) {
        clone = new (this instanceof HTMLDocument ? HTMLDocument : this instanceof XMLDocument ? XMLDocument : Document)();
        slots(clone).contentType = slots(this).contentType;
        slots(clone).url = slots(this).url;
      }
      else if (this instanceof DocumentFragment) clone = document.createDocumentFragment();
      else if (this instanceof Element) {
        const qualifiedName = this.prefix ? `${this.prefix}:${this.localName}` : this.localName;
        // The is value is part of what the element *is*, so a clone of a
        // customized built-in is one too, and upgrades like one.
        const is = isValueOf(this);
        clone = document.createElementNS(this.namespaceURI, qualifiedName, is === null ? undefined : { is });
        for (const attribute of ownAttributes(this)) {
          clone.setAttributeNS(attribute.namespaceURI, attribute.name, attribute.value);
        }
      } else if (this instanceof CDATASection) clone = new CDATASection(this.data, document);
      else if (this instanceof Text) clone = document.createTextNode(this.data);
      else if (this instanceof Comment) clone = document.createComment(this.data);
      else if (this instanceof ProcessingInstruction) clone = document.createProcessingInstruction(this.target, this.data);
      else if (this instanceof DocumentType) clone = new DocumentType(this.name, this.publicId, this.systemId, document);
      else throw domError("NotSupportedError", "This node cannot be cloned.");
      if (deep) {
        const source = this instanceof HTMLTemplateElement ? this.content : this;
        const target = clone instanceof HTMLTemplateElement ? clone.content : clone;
        for (const child of source._esdevChildren()) target._insert(child.cloneNode(true), null);
        // A shadow root comes along only when it said it could: `clonable`.
        const shadow = this[SHADOW_ROOT];
        if (shadow?.clonable) {
          const copy = clone.attachShadow({
            mode: shadow.mode,
            delegatesFocus: shadow.delegatesFocus,
            clonable: true,
            serializable: shadow.serializable,
            slotAssignment: shadow.slotAssignment,
          });
          for (const child of shadow._esdevChildren()) copy._insert(child.cloneNode(true), null);
        }
      }
      return clone;
    }
  }

  class CharacterData extends Node {
    constructor(type, name, data, ownerDocument) {
      super(type, name, ownerDocument);
      Object.defineProperty(this, DATA, { value: String(data), writable: true });
    }
    get data() { return this[DATA]; }
    set data(value) { this.replaceData(0, this.length, value === null ? "" : String(value)); }
    get nodeValue() { return this.data; }
    set nodeValue(value) { this.data = String(value ?? ""); }
    get length() { return this[DATA].length; }

    // Every edit is the specification's "replace data": one mutation record, and
    // live ranges inside the node moved the way the edit moved the text. Offsets
    // are UTF-16 code units, which is what a JavaScript string counts in.
    replaceData(offset, count, data) {
      offset = toUnsignedLong(offset);
      count = toUnsignedLong(count);
      data = String(data);
      const oldValue = this[DATA];
      if (offset > oldValue.length) throw new DOMException("The offset is past the end of the data.", "IndexSizeError");
      count = Math.min(count, oldValue.length - offset);
      this.ownerDocument?._queueMutation?.({ type: "characterData", target: this, oldValue });
      this[DATA] = oldValue.slice(0, offset) + data + oldValue.slice(offset + count);
      this.ownerDocument?._adjustRanges?.replaceData(this, offset, count, data.length);
    }
    substringData(offset, count) {
      offset = toUnsignedLong(offset);
      count = toUnsignedLong(count);
      if (offset > this.length) throw new DOMException("The offset is past the end of the data.", "IndexSizeError");
      return this[DATA].slice(offset, offset + count);
    }
    appendData(data) { this.replaceData(this.length, 0, String(data)); }
    insertData(offset, data) { this.replaceData(offset, 0, String(data)); }
    deleteData(offset, count) { this.replaceData(offset, count, ""); }
  }

  // Web IDL's `unsigned long`: modulo 2^32, so `-1` is 4294967295 and an offset
  // past the end, not a count from it.
  const toUnsignedLong = (value) => {
    const number = Number(value);
    return Number.isFinite(number) ? Math.trunc(number) >>> 0 : 0;
  };

  // `new Text()`, `new Comment()` and `new DocumentFragment()` take no document
  // in Web IDL: they belong to the current global's associated document, which
  // the window installs here. Without it they had no `ownerDocument` at all,
  // and the first `appendChild` into a tree failed on adoption.
  let currentDocument = null;

  function setCurrentDocument(document) {
    currentDocument = document;
    return document;
  }

  class Text extends CharacterData {
    constructor(data = "", ownerDocument = currentDocument) { super(Node.TEXT_NODE, "#text", data, ownerDocument); }

    splitText(offset) {
      offset = toUnsignedLong(offset);
      const length = this.length;
      if (offset > length) throw new DOMException("The offset is past the end of the data.", "IndexSizeError");
      const tail = new this.constructor(this.substringData(offset, length - offset), this.ownerDocument);
      const parent = this.parentNode;
      // Boundaries past the split go with the tail before the data is cut, or
      // cutting it would pull them back to the split point.
      this.ownerDocument?._adjustRanges?.splitText(this, tail, offset);
      // Chrome records the cut before the insertion, where the specification
      // inserts first; the records are what a test observes, so Chrome's order.
      this.replaceData(offset, length - offset, "");
      if (parent) {
        parent.insertBefore(tail, this.nextSibling);
        this.ownerDocument?._adjustRanges?.afterSplit(parent, childIndex(this));
      }
      return tail;
    }

    // The text of this node and every contiguous Text sibling either side.
    get wholeText() {
      let first = this;
      while (first.previousSibling instanceof Text) first = first.previousSibling;
      let text = "";
      for (let at = first; at instanceof Text; at = at.nextSibling) text += at.data;
      return text;
    }
  }

  class Comment extends CharacterData {
    constructor(data = "", ownerDocument = currentDocument) { super(Node.COMMENT_NODE, "#comment", data, ownerDocument); }
  }

  // HTML documents never contain CDATA sections, so `createCDATASection` refuses
  // in one. The interface is still exposed: code that branches on
  // `node instanceof CDATASection` should find a class, not a ReferenceError.
  class CDATASection extends Text {
    constructor(data, ownerDocument) {
      super(data, ownerDocument);
      slots(this).type = Node.CDATA_SECTION_NODE;
      slots(this).name = "#cdata-section";
    }
  }

  class ProcessingInstruction extends CharacterData {
    constructor(target, data, ownerDocument) {
      super(Node.PROCESSING_INSTRUCTION_NODE, String(target), data, ownerDocument);
    }
    get target() { return slots(this).name; }
  }

  class DocumentType extends Node {
    constructor(name, publicId, systemId, ownerDocument) {
      super(Node.DOCUMENT_TYPE_NODE, String(name), ownerDocument);
      defineIdl(this, {
        name: { value: String(name), enumerable: true },
        publicId: { value: String(publicId ?? ""), enumerable: true },
        systemId: { value: String(systemId ?? ""), enumerable: true },
      });
    }
    get nodeValue() { return null; }
    set nodeValue(_value) {}
    get textContent() { return null; }
    set textContent(_value) {}
  }

  class Attr extends Node {
    // `name` is the local name when a prefix is given, and otherwise the whole
    // qualified name, split at its first colon only if it has a namespace.
    constructor(name, value, ownerDocument, namespaceURI = null, prefix = undefined) {
      namespaceURI = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
      if (prefix === undefined) {
        const separator = name.indexOf(":");
        prefix = namespaceURI === null || separator === -1 ? null : name.slice(0, separator);
        if (prefix !== null) name = name.slice(separator + 1);
      }
      const qualifiedName = prefix === null ? name : `${prefix}:${name}`;
      super(Node.ATTRIBUTE_NODE, qualifiedName, ownerDocument);
      this.name = qualifiedName;
      this.value = String(value);
      this.ownerElement = null;
      this.namespaceURI = namespaceURI;
      this.prefix = prefix;
      this.localName = name;
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
      // Setting an attribute adopts it: it belongs to the element's document
      // afterwards, not to the one it came from.
      const document = this[ATTRS]?.ownerDocument;
      if (document) slots(attribute).ownerDocument = document;
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
  }
  NamedNodeMap.prototype[Symbol.iterator] = Array.prototype.values;

  class Element extends Node {
    // `name` is the local name: a prefix is only ever given by
    // `createElementNS`, which sets it afterwards.
    constructor(name, ownerDocument, namespaceURI = HTML_NAMESPACE) {
      super(Node.ELEMENT_NODE, name, ownerDocument);
      this.namespaceURI = namespaceURI;
      this.prefix = null;
      this.localName = name;
      slots(this).attributes = [];
      slots(this).children = null;
      Object.defineProperty(this, NAMED_ATTRIBUTES, { value: new NamedNodeMap(this) });
      Object.defineProperty(this, CLASS_LIST, { value: new DOMTokenList(this) });
      Object.defineProperty(this, DATASET, { value: new DOMStringMap(this) });
    }
    get attributes() { return this[NAMED_ATTRIBUTES]; }
    // The qualified name, uppercased only for an HTML element in an HTML
    // document — so it follows the element if it is adopted into an XML one.
    get tagName() {
      const name = this.prefix === null ? this.localName : `${this.prefix}:${this.localName}`;
      return this.namespaceURI === HTML_NAMESPACE && isHTMLDocument(this.ownerDocument) ? asciiUppercase(name) : name;
    }
    get nodeName() { return this.tagName; }
    getAttribute(name) { return ownAttributes(this).getNamedItem(qualified(this, name))?.value ?? null; }
    getAttributeNames() { return Array.from(ownAttributes(this), (attribute) => attribute.name); }
    getAttributeNode(name) { return ownAttributes(this).getNamedItem(qualified(this, name)); }
    hasAttribute(name) { return this.getAttributeNode(name) !== null; }
    toggleAttribute(name, force) {
      if (!isValidAttributeLocalName(String(name))) throw domError("InvalidCharacterError", `"${name}" is not a valid attribute name.`);
      name = qualified(this, name);
      const present = this.hasAttribute(name);
      if (force === undefined ? !present : Boolean(force)) {
        if (!present) this.setAttribute(name, "");
        return true;
      }
      if (present) this.removeAttribute(name);
      return false;
    }
    getAttributeNS(namespaceURI, localName) { return this.getAttributeNodeNS(namespaceURI, localName)?.value ?? null; }
    getAttributeNodeNS(namespaceURI, localName) { return ownAttributes(this).getNamedItemNS(namespaceURI, localName); }
    hasAttributeNS(namespaceURI, localName) { return this.getAttributeNodeNS(namespaceURI, localName) !== null; }
    setAttribute(name, value) {
      if (!isValidAttributeLocalName(String(name))) throw domError("InvalidCharacterError", `"${name}" is not a valid attribute name.`);
      name = qualified(this, name);
      const oldValue = this.getAttribute(name);
      // Captured before the change: a node that moves between slots signals the
      // one it left and then the one it joined, in that order.
      const before = name === "slot" || name === "name" ? assignedSlotFor(this) : null;
      ownAttributes(this).setNamedItem(new Attr(name, value, this.ownerDocument)); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: name, oldValue });
      attributeChanged(this, name, null, oldValue, this.getAttribute(name));
      if (name === "disabled") notifyDisabled(this);
      if (name === "slot" || name === "name") signalReassignment(this, before);
    }
    setAttributeNS(namespaceURI, qualifiedName, value) {
      const { namespace, prefix, localName } = validateAndExtract(namespaceURI, qualifiedName, "attribute");
      qualifiedName = prefix === null ? localName : `${prefix}:${localName}`;
      const attribute = new Attr(localName, value, this.ownerDocument, namespace, prefix);
      const oldValue = this.getAttributeNS(attribute.namespaceURI, attribute.localName);
      ownAttributes(this).setNamedItemNS(attribute); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: qualifiedName, oldValue });
      attributeChanged(this, attribute.localName, attribute.namespaceURI, oldValue, attribute.value);
    }
    setAttributeNode(attribute) {
      const previous = ownAttributes(this).setNamedItem(attribute); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: previous?.value ?? null });
      attributeChanged(this, attribute.localName, attribute.namespaceURI, previous?.value ?? null, attribute.value);
      return previous;
    }
    removeAttribute(name) {
      name = qualified(this, name);
      const attribute = this.getAttributeNode(name);
      const before = name === "slot" || name === "name" ? assignedSlotFor(this) : null;
      if (attribute) {
        ownAttributes(this).removeNamedItem(name); this._touch();
        this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: attribute.value });
        attributeChanged(this, attribute.localName, attribute.namespaceURI, attribute.value, null);
        if (name === "disabled") notifyDisabled(this);
        if (name === "slot" || name === "name") signalReassignment(this, before);
      }
    }
    removeAttributeNS(namespaceURI, localName) {
      const attribute = this.getAttributeNodeNS(namespaceURI, localName);
      if (attribute) {
        ownAttributes(this).removeNamedItemNS(namespaceURI, localName); this._touch();
        this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: attribute.name, oldValue: attribute.value });
        attributeChanged(this, attribute.localName, attribute.namespaceURI, attribute.value, null);
      }
    }
    removeAttributeNode(attribute) {
      if (attribute.ownerElement !== this) throw domError("NotFoundError", "The attribute is not owned by this element.");
      const removed = ownAttributes(this).removeNamedItem(attribute.name); this._touch();
      this.ownerDocument?._queueMutation?.({ type: "attributes", target: this, attributeName: removed.name, oldValue: removed.value });
      attributeChanged(this, removed.localName, removed.namespaceURI, removed.value, null);
      return removed;
    }
    get id() { return this.getAttribute("id") ?? ""; }
    set id(value) { this.setAttribute("id", value); }
    get className() { return this.getAttribute("class") ?? ""; }
    set className(value) { this.setAttribute("class", value); }
    get classList() { return this[CLASS_LIST]; }
    set classList(value) { this[CLASS_LIST].value = value; }
    get dataset() { return this[DATASET]; }
    get children() {
      const state = slots(this);
      return state.children ??= new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((node) => node instanceof Element));
    }
    get firstElementChild() { return this.children.item(0); }
    get lastElementChild() { return this.children.item(this.children.length - 1); }
    get childElementCount() { return this.children.length; }
    getElementsByTagName(name) { return elementsByQualifiedName(this, String(name)); }
    getElementsByTagNameNS(namespace, localName) { return elementsByNamespace(this, namespace, localName); }
    getElementsByClassName(names) {
      const expected = String(names).split(/[\t\n\f\r ]+/).filter(Boolean);
      return new HTMLCollection(this, (root) => collect(root, (element) => {
        const classes = new Set((element.getAttribute("class") ?? "").split(/[\t\n\f\r ]+/).filter(Boolean));
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
      if (name === undefined || ownerDocument === undefined) {
        // `new SomeElement()` from script: the element takes the name its class
        // was defined under, which is what the registry knows.
        const defined = customLookup?.(new.target);
        if (!defined) throw new TypeError("Illegal constructor");
        super(defined.name, defined.document);
        // A class defined with `{ extends: "button" }` constructs a `button`
        // that *is* the definition, not an element named after it.
        if (defined.is !== undefined) slots(this).isValue = defined.is;
        return;
      }
      super(name, ownerDocument);
    }
    click() {
      this.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
    }
  }

  // Namespace-specific classes are observable browser API, even when this
  // layout-free DOM has no SVG or MathML rendering behaviour of its own. SVG
  // gives every element name its own interface over a handful of shared bases,
  // and a renderer reads that: `<g>` is an `SVGGElement`, not a bare
  // `SVGElement`. The table below is Chrome's, element for element.
  class SVGElement extends Element {}
  class SVGGraphicsElement extends SVGElement {}
  class SVGGeometryElement extends SVGGraphicsElement {}
  class SVGTextContentElement extends SVGGraphicsElement {}
  class SVGTextPositioningElement extends SVGTextContentElement {}
  class SVGGradientElement extends SVGElement {}
  class SVGComponentTransferFunctionElement extends SVGElement {}
  class SVGAnimationElement extends SVGElement {}
  class MathMLElement extends Element {}

  const SVG_INTERFACES = {
    SVGElement,
    SVGGraphicsElement,
    SVGGeometryElement,
    SVGTextContentElement,
    SVGTextPositioningElement,
    SVGGradientElement,
    SVGComponentTransferFunctionElement,
    SVGAnimationElement,
  };
  // Local name to interface, grouped by the base the interface extends. The
  // leaf classes are generated rather than declared one by one, because nothing
  // about them differs but their name and their base.
  const SVG_ELEMENT_CLASSES = { __proto__: null };
  for (const [base, table] of [
    [SVGGraphicsElement, {
      a: "SVGAElement", defs: "SVGDefsElement", foreignObject: "SVGForeignObjectElement",
      g: "SVGGElement", image: "SVGImageElement", svg: "SVGSVGElement",
      switch: "SVGSwitchElement", symbol: "SVGSymbolElement", use: "SVGUseElement",
    }],
    [SVGGeometryElement, {
      circle: "SVGCircleElement", ellipse: "SVGEllipseElement", line: "SVGLineElement",
      path: "SVGPathElement", polygon: "SVGPolygonElement", polyline: "SVGPolylineElement",
      rect: "SVGRectElement",
    }],
    [SVGTextPositioningElement, { text: "SVGTextElement", tspan: "SVGTSpanElement" }],
    [SVGTextContentElement, { textPath: "SVGTextPathElement" }],
    [SVGGradientElement, {
      linearGradient: "SVGLinearGradientElement", radialGradient: "SVGRadialGradientElement",
    }],
    [SVGComponentTransferFunctionElement, {
      feFuncA: "SVGFEFuncAElement", feFuncB: "SVGFEFuncBElement",
      feFuncG: "SVGFEFuncGElement", feFuncR: "SVGFEFuncRElement",
    }],
    [SVGAnimationElement, {
      animate: "SVGAnimateElement", animateMotion: "SVGAnimateMotionElement",
      animateTransform: "SVGAnimateTransformElement", mpath: "SVGMPathElement",
      set: "SVGSetElement",
    }],
    [SVGElement, {
      clipPath: "SVGClipPathElement", desc: "SVGDescElement", feBlend: "SVGFEBlendElement",
      feColorMatrix: "SVGFEColorMatrixElement", feComponentTransfer: "SVGFEComponentTransferElement",
      feComposite: "SVGFECompositeElement", feConvolveMatrix: "SVGFEConvolveMatrixElement",
      feDiffuseLighting: "SVGFEDiffuseLightingElement", feDisplacementMap: "SVGFEDisplacementMapElement",
      feDistantLight: "SVGFEDistantLightElement", feDropShadow: "SVGFEDropShadowElement",
      feFlood: "SVGFEFloodElement", feGaussianBlur: "SVGFEGaussianBlurElement",
      feImage: "SVGFEImageElement", feMerge: "SVGFEMergeElement", feMergeNode: "SVGFEMergeNodeElement",
      feMorphology: "SVGFEMorphologyElement", feOffset: "SVGFEOffsetElement",
      fePointLight: "SVGFEPointLightElement", feSpecularLighting: "SVGFESpecularLightingElement",
      feSpotLight: "SVGFESpotLightElement", feTile: "SVGFETileElement",
      feTurbulence: "SVGFETurbulenceElement", filter: "SVGFilterElement",
      marker: "SVGMarkerElement", mask: "SVGMaskElement", metadata: "SVGMetadataElement",
      pattern: "SVGPatternElement", script: "SVGScriptElement", stop: "SVGStopElement",
      style: "SVGStyleElement", title: "SVGTitleElement", view: "SVGViewElement",
    }],
  ]) {
    for (const [element, name] of Object.entries(table)) {
      // The computed key names the class, so `constructor.name` and the
      // `Symbol.toStringTag` taken from it read as the interface, not as "".
      SVG_INTERFACES[name] ??= { [name]: class extends base {} }[name];
      SVG_ELEMENT_CLASSES[element] = SVG_INTERFACES[name];
    }
  }
  const { SVGSVGElement } = SVG_INTERFACES;

  class HTMLInputElement extends HTMLElement {
    constructor(name, ownerDocument) {
      super(name, ownerDocument);
      this[INPUT_VALUE] = null; this[INPUT_CHECKED] = null; this[INPUT_INDETERMINATE] = false;
      this[INPUT_SELECTION_START] = null; this[INPUT_SELECTION_END] = null; this[INPUT_SELECTION_DIRECTION] = "none";
    }
    click() {
      if (this.disabled) return;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      if (this.type === "checkbox") this.checked = !this.checked;
      if (this.type === "radio" && !this.checked) this.checked = true;
      if (this.type === "submit") this.form?.requestSubmit(this);
      if (this.type === "reset") this.form?.reset();
    }
  }

  // A radio button group holds at most one checked button, and the invariant
  // belongs to checkedness itself rather than to `click()`: setting `checked`
  // directly has to uncheck the others, which is what a form serializer sees.
  function unsetOtherRadios(input) {
    if (input.type !== "radio") return;
    const name = input.name;
    if (!name) return;
    const owner = formOwner(input);
    const root = input.getRootNode();
    const scope = root instanceof Document || root instanceof ShadowRoot ? root : input.ownerDocument;
    for (const other of collect(scope, (element) => element instanceof HTMLInputElement)) {
      if (other === input || other.type !== "radio" || other.name !== name) continue;
      if (formOwner(other) !== owner) continue;
      other[INPUT_CHECKED] = false;
    }
  }

  // `command` and `commandfor`: the button's activation behaviour dispatches a
  // CommandEvent at the element it names and performs the built-in command.
  function runCommand(button) {
    const id = button.getAttribute("commandfor");
    if (id === null) return;
    const root = button.getRootNode();
    const target = root?.getElementById?.(id) ?? button.ownerDocument.getElementById(id);
    if (!target) return;
    const command = (button.getAttribute("command") ?? "").trim();
    const event = new CommandEvent("command", { bubbles: false, cancelable: true, source: button, command });
    if (!target.dispatchEvent(event)) return;
    const named = command.toLowerCase();
    if (named === "show-modal") target.showModal?.();
    else if (named === "close") target.close?.();
    else if (named === "request-close") target.requestClose?.();
    else if (named === "show-popover") target.showPopover?.();
    else if (named === "hide-popover") target.hidePopover?.();
    else if (named === "toggle-popover") target.togglePopover?.();
  }

  class HTMLButtonElement extends HTMLElement {
    click() {
      if (this.disabled) return;
      const event = new MouseEvent("click", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      runCommand(this);
      if (this.type === "submit") this.form?.requestSubmit(this);
      if (this.type === "reset") this.form?.reset();
    }
  }

  // `close` and `toggle` are fired from a queued task, not from the call that
  // caused them — which is observable: neither has happened yet when the
  // function that opened or closed the thing returns.
  function queueElementTask(callback) {
    setTimeout(callback, 0);
  }

  // `beforetoggle` is synchronous, and only opening can be cancelled.
  function fireBeforeToggle(element, oldState, newState) {
    const event = new ToggleEvent("beforetoggle", { oldState, newState, cancelable: newState === "open" });
    return element.dispatchEvent(event);
  }

  // One pending `toggle` per element: a second change before it fires updates
  // the state it reports rather than queueing another, so opening and closing
  // in one task reports `closed` → `closed`, once. Chrome does this for all
  // three of popover, dialog and `<details>`.
  const PENDING_TOGGLE = Symbol("pendingToggle");
  function queueToggle(element, oldState, newState) {
    const pending = element[PENDING_TOGGLE];
    if (pending) {
      pending.newState = newState;
      return;
    }
    const task = { oldState, newState };
    element[PENDING_TOGGLE] = task;
    queueElementTask(() => {
      element[PENDING_TOGGLE] = null;
      element.dispatchEvent(new ToggleEvent("toggle", task));
    });
  }

  // The specification's "attribute change steps", run after every change to an
  // attribute however it was made.
  function attributeChanged(element, localName, namespace, oldValue, value) {
    if (namespace !== null) return;
    if (element instanceof HTMLDetailsElement) detailsAttributeChanged(element, localName, oldValue, value);
  }

  // A `<details>` fires `toggle` whenever its `open` attribute appears or goes
  // — by script, by attribute, attached or not — and never `beforetoggle`.
  // Opening one in a `name` group closes the others in the same tree.
  function detailsAttributeChanged(details, localName, oldValue, value) {
    if (localName !== "open" || (oldValue === null) === (value === null)) return;
    queueToggle(details, oldValue === null ? "closed" : "open", value === null ? "closed" : "open");
    const group = details.getAttribute("name");
    if (value === null || !group) return;
    for (const other of collect(details.getRootNode(), (element) => element instanceof HTMLDetailsElement)) {
      if (other !== details && other.getAttribute("name") === group && other.hasAttribute("open")) other.removeAttribute("open");
    }
  }

  class HTMLDetailsElement extends HTMLElement {}

  // A dialog's *state* needs no rendering: what is open, what it returned, and
  // which events that produced. There is no top layer and no backdrop here, so
  // `showModal` differs from `show` in the modal flag and in nothing visual.
  class HTMLDialogElement extends HTMLElement {
    // Showing asks first — a cancelable `beforetoggle` — and closing announces
    // itself; each queues a `toggle`, and closing a `close` after it. Setting
    // the `open` attribute directly does neither, as in Chrome.
    show() {
      if (this.open) return;
      if (!fireBeforeToggle(this, "closed", "open") || this.open) return;
      this.setAttribute("open", "");
      this[DIALOG_MODAL] = false;
      queueToggle(this, "closed", "open");
    }
    showModal() {
      if (this.open) {
        throw domError("InvalidStateError", "This dialog is already open.");
      }
      if (!this.isConnected) {
        throw domError("InvalidStateError", "A dialog must be in the document to be shown modally.");
      }
      if (!fireBeforeToggle(this, "closed", "open") || this.open) return;
      this.setAttribute("open", "");
      this[DIALOG_MODAL] = true;
      queueToggle(this, "closed", "open");
    }
    close(returnValue) {
      if (!this.open) return;
      fireBeforeToggle(this, "open", "closed");
      if (!this.open) return;
      if (returnValue !== undefined) this.returnValue = String(returnValue);
      this.removeAttribute("open");
      this[DIALOG_MODAL] = false;
      queueToggle(this, "open", "closed");
      queueElementTask(() => this.dispatchEvent(new Event("close")));
    }
    requestClose(returnValue) {
      if (!this.open) return;
      const event = new Event("cancel", { cancelable: true });
      if (!this.dispatchEvent(event)) return;
      this.close(returnValue);
    }
    get returnValue() { return this[DIALOG_RETURN] ?? ""; }
    set returnValue(value) { this[DIALOG_RETURN] = String(value); }
    _esdevIsModal() { return this[DIALOG_MODAL] === true && this.open; }
  }
  class HTMLDivElement extends HTMLElement {}
  class HTMLCanvasElement extends HTMLElement {}
  class HTMLAnchorElement extends HTMLElement {}
  class HTMLProgressElement extends HTMLElement {
    get value() {
      const value = Number(this.getAttribute("value"));
      return Number.isFinite(value) && value >= 0 ? value : 0;
    }
    set value(value) { this.setAttribute("value", String(Number(value))); }
    get max() {
      const value = Number(this.getAttribute("max"));
      return Number.isFinite(value) && value > 0 ? value : 1;
    }
    set max(value) { this.setAttribute("max", String(Number(value))); }
  }
  // The table family. `rows` is in the specification's order rather than tree
  // order: head first, then the bodies, then the foot, wherever the markup put
  // them — a `<tfoot>` written before a `<tbody>` still comes last.
  class HTMLTableElement extends HTMLElement {
    #sections(name) {
      return Array.from(this._esdevChildren()).filter((child) => child.localName === name);
    }
    get caption() { return this.#sections("caption")[0] ?? null; }
    get tHead() { return this.#sections("thead")[0] ?? null; }
    get tFoot() { return this.#sections("tfoot")[0] ?? null; }
    get tBodies() {
      return new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((child) => child.localName === "tbody"));
    }
    get rows() {
      return new HTMLCollection(this, (root) => {
        const rows = (parent) => Array.from(parent._esdevChildren()).filter((child) => child instanceof HTMLTableRowElement);
        const sections = Array.from(root._esdevChildren());
        return [
          ...sections.filter((child) => child.localName === "thead").flatMap(rows),
          ...rows(root),
          ...sections.filter((child) => child.localName === "tbody").flatMap(rows),
          ...sections.filter((child) => child.localName === "tfoot").flatMap(rows),
        ];
      });
    }
  }

  class HTMLTableSectionElement extends HTMLElement {
    get rows() {
      return new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((child) => child instanceof HTMLTableRowElement));
    }
  }

  class HTMLTableRowElement extends HTMLElement {
    get cells() {
      return new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((child) => child instanceof HTMLTableCellElement));
    }
    get rowIndex() {
      // Walked rather than `closest("table")`: the tree must not depend on the
      // selector engine having been installed over it.
      let table = this.parentElement;
      while (table && !(table instanceof HTMLTableElement)) table = table.parentElement;
      return table ? Array.from(table.rows).indexOf(this) : -1;
    }
    get sectionRowIndex() {
      const parent = this.parentElement;
      return parent instanceof HTMLTableSectionElement || parent instanceof HTMLTableElement
        ? Array.from(parent.rows).indexOf(this)
        : -1;
    }
  }

  class HTMLTableCellElement extends HTMLElement {
    get cellIndex() {
      const row = this.parentElement;
      return row instanceof HTMLTableRowElement ? Array.from(row.cells).indexOf(this) : -1;
    }
  }

  class HTMLStyleElement extends HTMLElement {}
  class HTMLTableCaptionElement extends HTMLElement {}
  class HTMLTableColElement extends HTMLElement {}

  function isSubmitter(control) {
    return (control instanceof HTMLButtonElement || control instanceof HTMLInputElement) && control.type === "submit";
  }

  // A form-associated custom element declares itself with a static field, so no
  // registry lookup is needed: the constructor is on the element already.
  function isFormAssociated(element) {
    return element instanceof HTMLElement && element.constructor?.formAssociated === true && isDefined(element);
  }

  // What a control contributes to its form's entry list, or null for one that
  // contributes nothing. Built-in controls are handled by FormData itself; this
  // is the custom-element half.
  function formSubmissionValue(element) {
    if (!isFormAssociated(element) || isDisabled(element)) return null;
    const name = element.getAttribute("name");
    const value = element[FORM_VALUE];
    if (!name || value === undefined || value === null) return null;
    return { name, value };
  }

  function formOwner(control) {
    const id = control.getAttribute("form");
    if (id !== null) return Array.from(control.ownerDocument.getElementsByTagName("form")).find((form) => form.id === id) ?? null;
    for (let parent = control.parentElement; parent; parent = parent.parentElement) if (parent instanceof HTMLFormElement) return parent;
    return null;
  }

  function isDisabled(control) {
    if (control.disabled) return true;
    // A custom element has no `disabled` property unless it wrote one, and its
    // disabled state comes from the attribute, as the specification says.
    if (control instanceof Element && control.hasAttribute("disabled") && isFormAssociated(control)) return true;
    for (let parent = control.parentElement; parent; parent = parent.parentElement) {
      if (parent instanceof HTMLFieldSetElement || parent instanceof HTMLOptGroupElement) {
        if (parent.disabled) return true;
      }
    }
    return false;
  }

  class HTMLFormElement extends HTMLElement {
    get elements() {
      return new HTMLCollection(this, () => Array.from(this.ownerDocument.getElementsByTagName("*")).filter((element) => (["button", "fieldset", "input", "select", "textarea"].includes(element.localName) || isFormAssociated(element)) && formOwner(element) === this));
    }
    reset() {
      const event = new Event("reset", { bubbles: true, cancelable: true });
      if (!this.dispatchEvent(event)) return;
      for (const control of this.elements) {
        // Only the callback: the submission value is the element's own state,
        // and clearing it is what its `formResetCallback` is for. Verified
        // against Chrome, which keeps the value across a reset.
        if (isFormAssociated(control)) control.formResetCallback?.();
        if (control instanceof HTMLSelectElement) for (const option of control.options) option[SELECTED] = null;
        if (control instanceof HTMLTextAreaElement) control[TEXTAREA_VALUE] = null;
        if (control instanceof HTMLInputElement) { control[INPUT_VALUE] = null; control[INPUT_CHECKED] = null; }
      }
    }
    checkValidity() {
      return Array.from(this.elements, (control) => (control[INTERNALS] ?? control).checkValidity?.() ?? true).every(Boolean);
    }
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
    // Not a reflection of anything: `type` says which kind of select this is,
    // and a framework reads it to decide whether one value or many are in play.
    get type() { return this.multiple ? "select-multiple" : "select-one"; }
    get options() { return new HTMLCollection(this, (root) => collect(root, (element) => element instanceof HTMLOptionElement)); }
    get length() { return this.options.length; }
    set length(value) {
      value = Math.max(0, Math.trunc(Number(value) || 0));
      while (this.options.length > value) {
        const last = this.options.item(this.options.length - 1);
        last.parentNode._remove(last);
      }
      while (this.options.length < value) this._insert(this.ownerDocument.createElement("option"), null);
    }
    get selectedIndex() {
      const options = Array.from(this.options);
      const selected = options.findIndex((option) => option.selected);
      // A single-select initially selects its first option, but an explicit
      // selectedIndex = -1 or unmatched value must remain an empty selection.
      // The option selectedness flags distinguish that dirty state from an
      // untouched option list.
      return selected >= 0 || this.multiple || !options.every((option) => option[SELECTED] === null)
        ? selected
        : options.length ? 0 : -1;
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
        return chosen.length || this.multiple || !options.every((option) => option[SELECTED] === null)
          ? chosen
          : options.slice(0, 1);
      });
    }
    add(item, before = null) {
      if (!(item instanceof HTMLOptionElement)) throw new TypeError("select.add expects an option");
      if (typeof before === "number") before = this.options.item(before);
      if (before !== null && before.parentNode !== this) throw domError("NotFoundError", "The reference option is not in this select.");
      this._preInsert(item, before ?? null);
    }
    remove(index) {
      const option = this.options.item(Number(index));
      option?.parentNode?._remove(option);
    }
  }

  class HTMLTextAreaElement extends HTMLElement {
    constructor(name, ownerDocument) { super(name, ownerDocument); this[TEXTAREA_VALUE] = null; }
    get defaultValue() { return this.textContent; }
    set defaultValue(value) { this.textContent = String(value); if (this[TEXTAREA_VALUE] === null) this[TEXTAREA_VALUE] = null; }
    get value() { return this[TEXTAREA_VALUE] ?? this.defaultValue; }
    set value(value) { this[TEXTAREA_VALUE] = String(value); }
  }

  class HTMLFieldSetElement extends HTMLElement {
    // A fieldset's controls are the listed elements it contains, which is not
    // the same question a form asks: a form follows the form owner, so a
    // control can belong to a form it is nowhere near.
    get elements() {
      return new HTMLCollection(this, () => Array.from(this.getElementsByTagName("*")).filter((element) => ["button", "fieldset", "input", "object", "output", "select", "textarea"].includes(element.localName)));
    }
  }
  class HTMLOptGroupElement extends HTMLElement {}

  // `[SameObject] readonly attribute ValidityState validity`: the object is the
  // same on every read and its flags are computed when they are asked for. A
  // fresh frozen record each time is neither, and `instanceof ValidityState`
  // fails on it.
  const VALIDITY_BRAND = Symbol("esdev DOM validity brand");
  const VALIDITY_FLAGS = [
    "badInput", "customError", "patternMismatch", "rangeOverflow", "rangeUnderflow",
    "stepMismatch", "tooLong", "tooShort", "typeMismatch", "valueMissing", "valid",
  ];

  class ValidityState {
    constructor(control, brand) {
      // The interface has no constructor in Web IDL, so script cannot make one.
      if (brand !== VALIDITY_BRAND) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, VALIDITY_CONTROL, { value: control });
    }
  }

  for (const flag of VALIDITY_FLAGS) {
    Object.defineProperty(ValidityState.prototype, flag, {
      get() { return validityFor(this[VALIDITY_CONTROL])[flag]; },
      enumerable: true,
      configurable: true,
    });
  }

  // Setlike, and the backing store `:state()` reads.
  class CustomStateSet {
    constructor(brand) {
      if (brand !== VALIDITY_BRAND) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, STATES, { value: new Set() });
    }
    get size() { return this[STATES].size; }
    add(state) { this[STATES].add(String(state)); return this; }
    delete(state) { return this[STATES].delete(String(state)); }
    has(state) { return this[STATES].has(String(state)); }
    clear() { this[STATES].clear(); }
    forEach(callback, thisArg) { for (const state of this[STATES]) callback.call(thisArg, state, state, this); }
    keys() { return this[STATES].keys(); }
    values() { return this[STATES].values(); }
    entries() { return this[STATES].entries(); }
    [Symbol.iterator]() { return this[STATES][Symbol.iterator](); }
  }

  const FORM_ONLY = "This element is not a form-associated custom element.";

  // The validity of a control, whether it is a built-in one or a custom element
  // that keeps its validity in its internals, for `:valid` to ask about.
  function controlValidity(element) {
    if (isFormAssociated(element)) return element[INTERNALS]?.validity ?? null;
    return element.validity ?? null;
  }

  // The states an element's internals declared, for `:state()` to match on.
  function customStates(element) {
    return element[INTERNALS]?.states?.[STATES] ?? null;
  }

  class ElementInternals {
    constructor(element, brand) {
      if (brand !== VALIDITY_BRAND) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, INTERNALS, { value: element });
      Object.defineProperty(this, STATES, { value: new CustomStateSet(VALIDITY_BRAND) });
    }
    // Unlike `element.shadowRoot`, this reaches a closed root: the element's
    // own implementation is the one party entitled to it.
    get shadowRoot() { return this[INTERNALS][SHADOW_ROOT] ?? null; }
    get states() { return this[STATES]; }
    #element() {
      const element = this[INTERNALS];
      if (!isFormAssociated(element)) throw domError("NotSupportedError", FORM_ONLY);
      return element;
    }
    get form() { return formOwner(this.#element()); }
    get labels() {
      const element = this.#element();
      return new HTMLCollection(element.ownerDocument, (root) => collect(root, (node) => node instanceof HTMLLabelElement && node.control === element));
    }
    get willValidate() { return !isDisabled(this.#element()); }
    get validity() {
      const element = this.#element();
      element[VALIDITY] ??= new ValidityState(element, VALIDITY_BRAND);
      return element[VALIDITY];
    }
    get validationMessage() { return this.#element()[FORM_VALIDITY]?.message ?? ""; }
    // `state` is kept for `formStateRestoreCallback`, which nothing in a test
    // realm triggers: there is no session history to restore from.
    setFormValue(value, state = undefined) {
      const element = this.#element();
      element[FORM_VALUE] = value ?? null;
      element[FORM_STATE] = state;
    }
    // The flags are the element's validity wholesale: a custom element decides
    // its own constraints, so nothing is computed from its attributes.
    setValidity(flags = {}, message = "", anchor = null) {
      const element = this.#element();
      const failing = Object.entries(flags).filter(([name, set]) => name !== "valid" && set).map(([name]) => name);
      if (failing.length > 0 && !String(message)) throw new TypeError("setValidity needs a message when a flag is set");
      element[FORM_VALIDITY] = failing.length === 0 ? null : { flags: Object.fromEntries(failing.map((name) => [name, true])), message: String(message), anchor };
    }
    checkValidity() {
      const element = this.#element();
      if (!this.willValidate || this.validity.valid) return true;
      element.dispatchEvent(new Event("invalid", { cancelable: true }));
      return false;
    }
    reportValidity() { return this.checkValidity(); }
  }

  function validityFor(control) {
    // A form-associated custom element has exactly the flags it set on itself.
    if (isFormAssociated(control)) {
      const state = control[FORM_VALIDITY];
      return Object.freeze({
        badInput: false, customError: false, patternMismatch: false, rangeOverflow: false, rangeUnderflow: false,
        stepMismatch: false, tooLong: false, tooShort: false, typeMismatch: false, valueMissing: false,
        ...(state?.flags ?? {}),
        valid: !state,
      });
    }
    return builtInValidityFor(control);
  }

  function builtInValidityFor(control) {
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
    defineIdl(Class.prototype, {
      willValidate: { get() { return !isDisabled(this); } },
      validity: { get() {
        this[VALIDITY] ??= new ValidityState(this, VALIDITY_BRAND);
        return this[VALIDITY];
      } },
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

  function isLabelable(element) {
    return ["button", "input", "select", "textarea"].includes(element.localName) || isFormAssociated(element);
  }

  function validDate(value) {
    const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
    if (!match) return false;
    const [year, month, day] = match.slice(1).map(Number);
    const date = new Date(Date.UTC(year, month - 1, day));
    return date.getUTCFullYear() === year && date.getUTCMonth() === month - 1 && date.getUTCDate() === day;
  }

  // A valid floating-point number, as HTML defines one: no leading or trailing
  // space, no hex, no `Infinity`.
  const FLOATING_POINT = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/;

  function floatingPoint(value) {
    if (!FLOATING_POINT.test(value)) return null;
    const number = Number(value);
    return Number.isFinite(number) ? number : null;
  }

  // The value sanitization algorithm, which *filters* — it does not rewrite. A
  // `number` input holding `1.00` reads back `1.00`, not `1`: the string form is
  // the author's, and a control that canonicalized it would fight every
  // framework that compares what it wrote with what it reads.
  function sanitizeInputValue(type, value) {
    value = String(value);
    if (type === "number") return floatingPoint(value) === null ? "" : value;
    if (type === "date") return validDate(value) ? value : "";
    return value;
  }

  // `range` is the exception, and the specification says so: its value is
  // *clamped* to the range and snapped to the nearest step, and a value that is
  // no number at all becomes the range's default — the midpoint, which is why an
  // empty range input reads `50`.
  function sanitizeRangeValue(element, value) {
    const minimum = floatingPoint(element.getAttribute("min") ?? "") ?? 0;
    const maximum = Math.max(minimum, floatingPoint(element.getAttribute("max") ?? "") ?? 100);
    const number = floatingPoint(String(value));
    if (number === null) {
      const middle = minimum + (maximum - minimum) / 2;
      return String(snapToStep(element, middle, minimum));
    }
    return String(snapToStep(element, Math.min(maximum, Math.max(minimum, number)), minimum));
  }

  function snapToStep(element, number, base) {
    const attribute = element.getAttribute("step") ?? "";
    if (attribute.trim().toLowerCase() === "any") return number;
    const step = floatingPoint(attribute) ?? 1;
    if (!(step > 0)) return number;
    // Halfway rounds up, as a browser does: step 2 from 0 makes 5 into 6.
    const steps = Math.floor((number - base) / step + 0.5);
    const snapped = base + steps * step;
    // Float arithmetic leaves 0.30000000000000004 behind; the step's own decimal
    // places are how many the result can have.
    const places = (String(step).split(".")[1] ?? "").length;
    return places === 0 ? snapped : Number(snapped.toFixed(places));
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

  function reflectInteger(attribute, fallback, minimum = Number.NEGATIVE_INFINITY, refusesZero = false) {
    return {
      get() {
        const value = Number.parseInt(this.getAttribute(attribute) ?? "", 10);
        return Number.isFinite(value) && value >= minimum ? value : fallback;
      },
      set(value) {
        // Web IDL converts to `unsigned long` first, and a property "limited to
        // only positive numbers" then refuses zero rather than clamping it: a
        // framework's warning path is written against that error, so silently
        // accepting `size = 0` hides the mistake it exists to report.
        if (refusesZero) {
          const unsigned = Number(value);
          const wrapped = Number.isFinite(unsigned) ? Math.trunc(unsigned) >>> 0 : 0;
          if (wrapped === 0) throw domError("IndexSizeError", `${attribute} cannot be zero.`);
          this.setAttribute(attribute, String(wrapped));
          return;
        }
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
    for (const [property, [attribute, fallback, minimum, refusesZero]] of Object.entries(integers)) {
      properties[property] = reflectInteger(attribute, fallback, minimum, refusesZero);
    }
    defineIdl(Class.prototype, properties);
  }

  // ARIAMixin: every one of these reflects to its attribute, and reads back
  // `null` rather than `""` when the attribute is absent. `internals`' copies
  // are deliberately *not* these — those are defaults the element carries
  // without writing anything into the markup.
  const ARIA_PROPERTIES = [
    "role", "ariaAtomic", "ariaAutoComplete", "ariaBrailleLabel", "ariaBrailleRoleDescription", "ariaBusy",
    "ariaChecked", "ariaColCount", "ariaColIndex", "ariaColSpan", "ariaCurrent", "ariaDescription",
    "ariaDisabled", "ariaExpanded", "ariaHasPopup", "ariaHidden", "ariaInvalid", "ariaKeyShortcuts",
    "ariaLabel", "ariaLevel", "ariaLive", "ariaModal", "ariaMultiLine", "ariaMultiSelectable",
    "ariaOrientation", "ariaPlaceholder", "ariaPosInSet", "ariaPressed", "ariaReadOnly", "ariaRelevant",
    "ariaRequired", "ariaRoleDescription", "ariaRowCount", "ariaRowIndex", "ariaRowSpan", "ariaSelected",
    "ariaSetSize", "ariaSort", "ariaValueMax", "ariaValueMin", "ariaValueNow", "ariaValueText",
  ];

  for (const property of ARIA_PROPERTIES) {
    const attribute = property === "role" ? "role" : `aria-${property.slice(4).toLowerCase()}`;
    Object.defineProperty(Element.prototype, property, {
      get() { return this.getAttribute(attribute); },
      set(value) {
        if (value === null || value === undefined) this.removeAttribute(attribute);
        else this.setAttribute(attribute, String(value));
      },
      enumerable: true,
      configurable: true,
    });
  }

  installReflectors(HTMLElement,
    { id: "id", className: "class", title: "title", lang: "lang", dir: "dir", slot: "slot" },
    { hidden: "hidden", inert: "inert" });
  installReflectors(HTMLInputElement,
    { accept: "accept", alt: "alt", autocomplete: "autocomplete", formEnctype: "formenctype", formMethod: "formmethod", formTarget: "formtarget", name: "name", placeholder: "placeholder" },
    { disabled: "disabled", formNoValidate: "formnovalidate", multiple: "multiple", readOnly: "readonly", required: "required" },
    { maxLength: ["maxlength", -1, -1], minLength: ["minlength", -1, -1], size: ["size", 20, 1, true] });
  installReflectors(HTMLButtonElement,
    { formEnctype: "formenctype", formMethod: "formmethod", formTarget: "formtarget", name: "name", value: "value" },
    { disabled: "disabled", formNoValidate: "formnovalidate" });
  installReflectors(HTMLDialogElement, {}, { open: "open" });
  installReflectors(HTMLDetailsElement, { name: "name" }, { open: "open" });
  installReflectors(HTMLCanvasElement, {}, {}, { width: ["width", 300, 0], height: ["height", 150, 0] });
  defineIdl(HTMLAnchorElement.prototype, { href: reflectUrl("href") });
  installReflectors(HTMLTableElement, { border: "border" });
  installReflectors(HTMLFormElement, { name: "name", target: "target" }, { noValidate: "novalidate" });
  installReflectors(HTMLLabelElement, { htmlFor: "for" });
  installReflectors(HTMLSelectElement,
    { name: "name" },
    { disabled: "disabled", multiple: "multiple", required: "required" },
    { size: ["size", 0, 0] });
  installReflectors(HTMLTextAreaElement,
    { name: "name", placeholder: "placeholder" },
    { disabled: "disabled", readOnly: "readonly", required: "required" },
    { cols: ["cols", 20, 1], rows: ["rows", 2, 1], maxLength: ["maxlength", -1, -1], minLength: ["minlength", -1, -1] });
  installReflectors(HTMLFieldSetElement, { name: "name" }, { disabled: "disabled" });
  installReflectors(HTMLOptGroupElement, { label: "label" }, { disabled: "disabled" });
  installValidation(HTMLInputElement);
  installValidation(HTMLSelectElement);
  installValidation(HTMLTextAreaElement);
  defineIdl(HTMLElement.prototype, {
    contentEditable: {
      get() {
        const value = this.getAttribute("contenteditable");
        return value === null ? "inherit" : ["true", "false", "plaintext-only"].includes(value.toLowerCase()) ? value.toLowerCase() : "inherit";
      },
      set(value) {
        value = String(value).toLowerCase();
        if (!["true", "false", "plaintext-only", "inherit"].includes(value)) throw new SyntaxError("contentEditable must be 'true', 'false', 'plaintext-only', or 'inherit'.");
        if (value === "inherit") this.removeAttribute("contenteditable"); else this.setAttribute("contenteditable", value);
      },
    },
    isContentEditable: {
      get() {
        for (let element = this; element; element = element.parentElement) {
          const value = element.contentEditable;
          if (value === "true" || value === "plaintext-only") return true;
          if (value === "false") return false;
        }
        return false;
      },
    },
    translate: {
      get() { return this.getAttribute("translate")?.toLowerCase() !== "no"; },
      set(value) { this.setAttribute("translate", value ? "yes" : "no"); },
    },
    draggable: {
      get() {
        const value = this.getAttribute("draggable");
        if (value !== null) return value.toLowerCase() === "true";
        return (this.localName === "a" || this.localName === "area") && this.hasAttribute("href") || this.localName === "img";
      },
      set(value) { this.setAttribute("draggable", value ? "true" : "false"); },
    },
    spellcheck: {
      get() { return this.getAttribute("spellcheck")?.toLowerCase() !== "false"; },
      set(value) { this.setAttribute("spellcheck", value ? "true" : "false"); },
    },
    tabIndex: {
      get() {
        const value = Number.parseInt(this.getAttribute("tabindex") ?? "", 10);
        if (Number.isFinite(value)) return value;
        return ["button", "input", "select", "textarea", "iframe"].includes(this.localName) || ["a", "area"].includes(this.localName) && this.hasAttribute("href") ? 0 : -1;
      },
      set(value) {
        value = Number(value);
        this.setAttribute("tabindex", String(Number.isFinite(value) ? Math.trunc(value) : -1));
      },
    },
  });
  defineIdl(HTMLInputElement.prototype, {
    type: { get() { return this.getAttribute("type") ?? "text"; }, set(value) { this.setAttribute("type", String(value)); } },
    value: {
      get() {
        const value = this[INPUT_VALUE] ?? this.defaultValue;
        return this.type === "range" ? sanitizeRangeValue(this, value) : sanitizeInputValue(this.type, value);
      },
      set(value) {
        this[INPUT_VALUE] = this.type === "range"
          ? sanitizeRangeValue(this, value)
          : sanitizeInputValue(this.type, value);
        if (selectionCapable(this)) {
          const end = this[INPUT_VALUE].length;
          this[INPUT_SELECTION_START] = end; this[INPUT_SELECTION_END] = end; this[INPUT_SELECTION_DIRECTION] = "none";
        }
      },
    },
    defaultValue: {
      get() { return this.getAttribute("value") ?? (["checkbox", "radio"].includes(this.type) ? "on" : ""); },
      set(value) { this.setAttribute("value", String(value)); },
    },
    checked: {
      get() { return this[INPUT_CHECKED] ?? this.defaultChecked; },
      set(value) {
        this[INPUT_CHECKED] = Boolean(value);
        if (this[INPUT_CHECKED]) unsetOtherRadios(this);
      },
    },
    indeterminate: { get() { return this[INPUT_INDETERMINATE]; }, set(value) { this[INPUT_INDETERMINATE] = Boolean(value); } },
    defaultChecked: { get() { return this.hasAttribute("checked"); }, set(value) { if (value) this.setAttribute("checked", ""); else this.removeAttribute("checked"); } },
    min: { get() { return this.getAttribute("min") ?? ""; }, set(value) { this.setAttribute("min", String(value)); } },
    max: { get() { return this.getAttribute("max") ?? ""; }, set(value) { this.setAttribute("max", String(value)); } },
    pattern: { get() { return this.getAttribute("pattern") ?? ""; }, set(value) { this.setAttribute("pattern", String(value)); } },
    step: { get() { return this.getAttribute("step") ?? ""; }, set(value) { this.setAttribute("step", String(value)); } },
    selectionStart: {
      get() { return selectionCapable(this) ? selectionRange(this)[0] : null; },
      set(value) { this.setSelectionRange(value, this.selectionEnd ?? value, this.selectionDirection); },
    },
    selectionEnd: {
      get() { return selectionCapable(this) ? selectionRange(this)[1] : null; },
      set(value) { this.setSelectionRange(this.selectionStart ?? value, value, this.selectionDirection); },
    },
    selectionDirection: {
      get() { return selectionCapable(this) ? this[INPUT_SELECTION_DIRECTION] : null; },
      set(value) { this.setSelectionRange(this.selectionStart ?? 0, this.selectionEnd ?? 0, value); },
    },
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

  function selectionCapable(input) { return ["text", "search", "tel", "url", "password"].includes(input.type); }
  function selectionRange(input) {
    const end = input.value.length;
    const start = input[INPUT_SELECTION_START] ?? end;
    return [Math.min(start, end), Math.min(input[INPUT_SELECTION_END] ?? end, end)];
  }
  HTMLInputElement.prototype.setSelectionRange = function(start, end, direction = "none") {
    if (!selectionCapable(this)) throw domError("InvalidStateError", "This input type does not support selection.");
    if (!["forward", "backward", "none"].includes(direction)) throw new TypeError("selection direction must be forward, backward, or none.");
    const length = this.value.length;
    start = Math.max(0, Math.min(length, Number(start)));
    end = Math.max(0, Math.min(length, Number(end)));
    if (end < start) start = end;
    this[INPUT_SELECTION_START] = start; this[INPUT_SELECTION_END] = end; this[INPUT_SELECTION_DIRECTION] = direction;
  };
  defineIdl(HTMLButtonElement.prototype, {
    type: { get() { return this.getAttribute("type") ?? "submit"; }, set(value) { this.setAttribute("type", String(value)); } },
  });
  defineIdl(HTMLFormElement.prototype, {
    action: reflectUrl("action"),
    method: { get() { return (this.getAttribute("method") ?? "get").toLowerCase(); }, set(value) { this.setAttribute("method", String(value).toLowerCase()); } },
    enctype: { get() { return this.getAttribute("enctype") ?? "application/x-www-form-urlencoded"; }, set(value) { this.setAttribute("enctype", String(value)); } },
  });
  defineIdl(HTMLButtonElement.prototype, { formAction: reflectUrl("formaction") });
  defineIdl(HTMLInputElement.prototype, { formAction: reflectUrl("formaction") });
  for (const Class of [HTMLInputElement, HTMLButtonElement, HTMLSelectElement, HTMLTextAreaElement]) {
    Object.defineProperty(Class.prototype, "form", { configurable: true, enumerable: true, get() { return formOwner(this); } });
    Object.defineProperty(Class.prototype, "labels", { configurable: true, enumerable: true,
      get() {
        return new HTMLCollection(this.ownerDocument, (root) => collect(root, (element) => element instanceof HTMLLabelElement && element.control === this));
      },
    });
  }

  // Geometry, as zeros. There is no box model, so every number here is 0 and
  // every scroll is a no-op — but the members exist, because feature-probing
  // code reads them before it does anything interesting and should not explode.
  // `docs/ESDEV-DOM.md` records this as the layout non-goal.
  class DOMRectReadOnly {
    constructor(x = 0, y = 0, width = 0, height = 0) {
      defineIdl(this, {
        x: { value: Number(x), enumerable: true },
        y: { value: Number(y), enumerable: true },
        width: { value: Number(width), enumerable: true },
        height: { value: Number(height), enumerable: true },
      });
    }
    get top() { return Math.min(this.y, this.y + this.height); }
    get bottom() { return Math.max(this.y, this.y + this.height); }
    get left() { return Math.min(this.x, this.x + this.width); }
    get right() { return Math.max(this.x, this.x + this.width); }
    toJSON() {
      const { x, y, width, height, top, right, bottom, left } = this;
      return { x, y, width, height, top, right, bottom, left };
    }
  }

  class DOMRect extends DOMRectReadOnly {
    static fromRect(other = {}) { return new DOMRect(other.x, other.y, other.width, other.height); }
  }

  const ZERO_METRICS = ["offsetWidth", "offsetHeight", "offsetTop", "offsetLeft", "clientWidth", "clientHeight", "clientTop", "clientLeft", "scrollWidth", "scrollHeight"];

  for (const name of ZERO_METRICS) {
    Object.defineProperty(Element.prototype, name, { get() { return 0; }, configurable: true });
  }
  defineIdl(Element.prototype, {
    // Assignable and still zero, which is what a browser answers for an element
    // that cannot scroll — and without layout, none of them can.
    scrollTop: { get() { return 0; }, set(_value) {}, configurable: true },
    scrollLeft: { get() { return 0; }, set(_value) {}, configurable: true },
    getBoundingClientRect: { value() { return new DOMRect(); }, writable: true, configurable: true },
    getClientRects: { value() { return Object.freeze([]); }, writable: true, configurable: true },
    scrollIntoView: { value() {}, writable: true, configurable: true },
    scroll: { value() {}, writable: true, configurable: true },
    scrollTo: { value() {}, writable: true, configurable: true },
    scrollBy: { value() {}, writable: true, configurable: true },
  });

  // The popover API, as state and events. There is no top layer to put anything
  // in, so what is observable is what is implemented: which popover is open,
  // `:popover-open`, and the `beforetoggle`/`toggle` events either side of the
  // change.
  const POPOVER_VALUES = new Set(["auto", "manual", "hint"]);

  function popoverKind(element) {
    const written = element.getAttribute("popover");
    if (written === null) return null;
    const value = written.toLowerCase();
    if (value === "") return "auto";
    return POPOVER_VALUES.has(value) ? value : "manual";
  }

  function toggleEvents(element, from, to) {
    if (!fireBeforeToggle(element, from, to)) return false;
    queueToggle(element, from, to);
    return true;
  }

  defineIdl(HTMLElement.prototype, {
    popover: {
      get() {
        const value = popoverKind(this);
        return value === null ? null : value;
      },
      set(value) {
        if (value === null || value === undefined) this.removeAttribute("popover");
        else this.setAttribute("popover", String(value));
      },
      enumerable: true,
      configurable: true,
    },
    showPopover: { value() {
      if (popoverKind(this) === null) throw domError("NotSupportedError", "This element is not a popover.");
      if (!this.isConnected) throw domError("InvalidStateError", "A popover must be in the document to be shown.");
      if (this[POPOVER_OPEN]) return;
      // An auto popover closes the other auto popovers, as only one light
      // dismiss stack exists.
      if (popoverKind(this) === "auto") {
        const document = this.ownerDocument;
        for (const other of collect(document, (element) => element[POPOVER_OPEN] === true)) {
          if (other !== this && popoverKind(other) === "auto") other.hidePopover();
        }
      }
      if (!toggleEvents(this, "closed", "open")) return;
      this[POPOVER_OPEN] = true;
    }, writable: true, configurable: true },
    hidePopover: { value() {
      if (popoverKind(this) === null) throw domError("NotSupportedError", "This element is not a popover.");
      if (!this[POPOVER_OPEN]) return;
      if (!toggleEvents(this, "open", "closed")) return;
      this[POPOVER_OPEN] = false;
    }, writable: true, configurable: true },
    togglePopover: { value(force) {
      const wanted = force === undefined ? !this[POPOVER_OPEN] : Boolean(force);
      if (wanted) this.showPopover();
      else this.hidePopover();
      return this[POPOVER_OPEN] === true;
    }, writable: true, configurable: true },
    _esdevPopoverOpen: { value() { return this[POPOVER_OPEN] === true; } },
  });

  Object.defineProperty(HTMLElement.prototype, "attachInternals", { configurable: true, enumerable: true,
    value() {
      // Only a custom element has internals, and only one set of them: a
      // built-in has nothing to attach, and a second call would hand a second
      // party the same element's private surface.
      const state = slots(this);
      const custom = state.customDefined === true || state.customPrecustomized === true;
      if (!custom || !CUSTOM_NAME.test(this.localName)) {
        throw domError("NotSupportedError", "Only a defined custom element has internals.");
      }
      if (this[INTERNALS]) throw domError("NotSupportedError", "This element already has internals attached.");
      const internals = new ElementInternals(this, VALIDITY_BRAND);
      Object.defineProperty(this, INTERNALS, { value: internals });
      return internals;
    },
    writable: true,
    configurable: true,
  });

  class DocumentFragment extends Node {
    constructor(ownerDocument = currentDocument) { super(Node.DOCUMENT_FRAGMENT_NODE, "#document-fragment", ownerDocument); }
    get children() {
      const state = slots(this);
      return state.children ??= new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((node) => node instanceof Element));
    }
    get firstElementChild() { return this.children.item(0); }
    get lastElementChild() { return this.children.item(this.children.length - 1); }
    get childElementCount() { return this.children.length; }
    // NonElementParentNode: a fragment and a shadow root answer this as a
    // document does, and it is the idiomatic call inside a shadow root.
    getElementById(id) {
      id = String(id);
      return collect(this, (element) => element.id === id)[0] ?? null;
    }
  }

  class ShadowRoot extends DocumentFragment {
    constructor(host, options) {
      super(host.ownerDocument);
      Object.defineProperty(this, SHADOW_OPTIONS, {
        value: {
          host,
          mode: options.mode,
          delegatesFocus: Boolean(options.delegatesFocus),
          clonable: Boolean(options.clonable),
          serializable: Boolean(options.serializable),
          slotAssignment: options.slotAssignment === "manual" ? "manual" : "named",
        },
      });
    }
    get host() { return this[SHADOW_OPTIONS].host; }
    get mode() { return this[SHADOW_OPTIONS].mode; }
    get delegatesFocus() { return this[SHADOW_OPTIONS].delegatesFocus; }
    get clonable() { return this[SHADOW_OPTIONS].clonable; }
    get serializable() { return this[SHADOW_OPTIONS].serializable; }
    get slotAssignment() { return this[SHADOW_OPTIONS].slotAssignment; }
    _eventParent(event) { return event.composed ? this.host : null; }
  }

  // The slot a node is assigned to, from the node's side. It is the same
  // question `slot.assignedNodes()` answers, asked the other way round, and a
  // component reads it to know whether it was projected at all.
  // `formDisabledCallback` is about the *computed* disabled state, so a
  // fieldset turning itself off has to tell the form-associated custom elements
  // inside it — and only the ones whose answer actually changed.
  function notifyDisabled(root) {
    const candidates = [root, ...collect(root, (element) => isFormAssociated(element))];
    for (const control of candidates) {
      if (!isFormAssociated(control)) continue;
      const state = isDisabled(control);
      if (control[FORM_DISABLED] === state) continue;
      control[FORM_DISABLED] = state;
      control.formDisabledCallback?.(state);
    }
  }

  function assignedSlotFor(node) {
    const host = node.parentElement;
    const root = host?.[SHADOW_ROOT];
    if (!root) return null;
    for (const slot of collect(root, (element) => element instanceof HTMLSlotElement)) {
      if (slot._assignedNodes().includes(node)) return slot;
    }
    return null;
  }

  class HTMLSlotElement extends HTMLElement {
    get name() { return this.getAttribute("name") ?? ""; }
    set name(value) { this.setAttribute("name", String(value)); }
    // In a manual-assignment root the `slot` attribute means nothing: a slot
    // holds what `assign()` gave it, and only those of its host's children.
    assign(...nodes) {
      this[MANUAL_ASSIGNED] = nodes.filter((node) => node instanceof Node);
    }
    _assignedNodes() {
      const root = this.getRootNode();
      if (!(root instanceof ShadowRoot)) return [];
      if (root.slotAssignment === "manual") {
        return (this[MANUAL_ASSIGNED] ?? []).filter((node) => node.parentNode === root.host);
      }
      const name = this.name;
      return Array.from(root.host._esdevChildren()).filter((node) => {
        const slot = node instanceof Element ? node.getAttribute("slot") ?? "" : "";
        return slot === name;
      });
    }
    assignedNodes(options = {}) {
      const assigned = this._assignedNodes();
      if (!options.flatten) return assigned;
      // "Find flattened slottables": the assigned nodes, or this slot's own
      // children when nothing is assigned — and then every one of *those* that is
      // itself a slot in a shadow tree dissolves into what it would show. A slot
      // forwarded into another component's slot is the ordinary case, and
      // returning it verbatim is what made `queryAssignedElements` answer with a
      // `<slot>` instead of the nodes behind it.
      const slottables = assigned.length > 0 ? assigned : Array.from(this._esdevChildren());
      const flattened = [];
      for (const node of slottables) {
        if (node instanceof HTMLSlotElement && node.getRootNode() instanceof ShadowRoot) {
          flattened.push(...node.assignedNodes({ flatten: true }));
        } else {
          flattened.push(node);
        }
      }
      return flattened;
    }
    assignedElements(options = {}) {
      return this.assignedNodes(options).filter((node) => node instanceof Element);
    }
    // Queued rather than fired: `slotchange` is delivered at the microtask
    // checkpoint, once, however many children moved.
    _signalChange() {
      if (this[SLOT_PENDING]) return;
      this[SLOT_PENDING] = true;
      queueMicrotask(() => {
        this[SLOT_PENDING] = false;
        this.dispatchEvent(new Event("slotchange", { bubbles: true }));
      });
    }
  }

  // A node whose `slot` changed, or a slot whose `name` did: the slot that lost
  // it hears first, then the one that took it.
  function signalReassignment(element, before) {
    if (element instanceof HTMLSlotElement) {
      signalSlotChange(element.parentElement);
      return;
    }
    const after = assignedSlotFor(element);
    if (before === after) return;
    before?._signalChange();
    after?._signalChange();
  }

  // Every slot whose assignment a change to `host`'s children could alter.
  function signalSlotChange(host) {
    const root = host?.[SHADOW_ROOT];
    if (!root) return;
    for (const slot of collect(root, (element) => element instanceof HTMLSlotElement)) slot._signalChange();
  }

  // The "template contents owner": a second, inert document that owns a
  // document's templates' contents. `template.content.ownerDocument` is *not*
  // the template's own document, and code reads that — a fragment moved out of a
  // template is adopted, while the fragment itself stays where it is. The owner
  // is its own owner, so a template inside template content does not start a
  // chain of documents.
  const TEMPLATE_OWNER = Symbol("esdev DOM template contents owner");

  function templateContentsOwner(document) {
    if (document === null || document === undefined) return document;
    let owner = document[TEMPLATE_OWNER];
    if (owner === undefined) {
      // Of the same kind as the template's document: an HTML document's
      // templates parse and create as HTML.
      owner = isHTMLDocument(document) ? new HTMLDocument() : new Document();
      Object.defineProperty(owner, TEMPLATE_OWNER, { value: owner });
      Object.defineProperty(document, TEMPLATE_OWNER, { value: owner });
    }
    return owner;
  }

  class HTMLTemplateElement extends HTMLElement {
    constructor(name, ownerDocument) {
      super(name, ownerDocument);
      this[TEMPLATE_CONTENT] = new DocumentFragment(templateContentsOwner(ownerDocument));
      // Inert: a template's content belongs to no browsing context, so a custom
      // element written inside one is not upgraded and is not `:defined` until
      // the content is cloned into a tree that is.
      this[TEMPLATE_CONTENT][INERT] = true;
    }

    // `[SameObject] readonly attribute DocumentFragment content`: an own data
    // property would be writable, enumerable and invisible to anything that
    // looks the interface up on the prototype.
    get content() { return this[TEMPLATE_CONTENT]; }
  }

  // The four insertion positions, resolved once: `insertAdjacentHTML` in
  // parse.js needs exactly the same validation and reference child, and the
  // errors are observable — an unknown position is a SyntaxError, and a
  // sibling insertion with no element parent is NoModificationAllowedError.
  Object.defineProperty(Node.prototype, "assignedSlot", {
    get() { return assignedSlotFor(this); },
    configurable: true,
  });

  defineIdl(Element.prototype, {
    _adjacentPosition: { value(where) {
      switch (String(where).toLowerCase()) {
        case "beforebegin":
        case "afterend": {
          const parent = this.parentNode;
          if (!parent || parent instanceof Document) {
            throw domError("NoModificationAllowedError", "There is no parent to insert a sibling into.");
          }
          return { parent, reference: where.toLowerCase() === "beforebegin" ? this : this.nextSibling, context: parent };
        }
        case "afterbegin":
          return { parent: this, reference: this.firstChild, context: this };
        case "beforeend":
          return { parent: this, reference: null, context: this };
        default:
          throw domError("SyntaxError", `'${where}' is not a valid insertion position.`);
      }
    } },
    insertAdjacentElement: { value(where, element) {
      if (!(element instanceof Element)) throw new TypeError("insertAdjacentElement expects an Element");
      const { parent, reference } = this._adjacentPosition(where);
      parent._preInsert(element, reference);
      return element;
    } },
    insertAdjacentText: { value(where, data) {
      const { parent, reference } = this._adjacentPosition(where);
      parent._preInsert((this.ownerDocument ?? this).createTextNode(String(data)), reference);
    } },
  });

  defineIdl(Element.prototype, {
    attachShadow: { value(options = {}) {
      // The specification's list, plus any valid custom element name. An
      // `<input>` cannot host a root, and a component that tries deserves to
      // hear so rather than to end up with a root nothing renders.
      if (!SHADOW_HOSTS.has(this.localName) && !(this.namespaceURI === HTML_NAMESPACE && CUSTOM_NAME.test(this.localName))) {
        throw domError("NotSupportedError", `<${this.localName}> cannot host a shadow root.`);
      }
      if (this[SHADOW_ROOT]) throw domError("NotSupportedError", "This element already hosts a shadow root.");
      const mode = options.mode;
      if (mode !== "open" && mode !== "closed") throw new TypeError("attachShadow requires mode 'open' or 'closed'.");
      const root = new ShadowRoot(this, options);
      this[SHADOW_ROOT] = root;
      return root;
    } },
    shadowRoot: { get() { const root = this[SHADOW_ROOT]; return root?.mode === "open" ? root : null; } },
    // The cascade needs a closed root too — `:host` styles it either way — and
    // a method rather than a property keeps it off the ordinary surface.
    _esdevShadowRoot: { value() { return this[SHADOW_ROOT] ?? null; } },
  });

  class Document extends Node {
    constructor() {
      super(Node.DOCUMENT_NODE, "#document", null);
      slots(this).ownerDocument = this;
      slots(this).version = 0;
      // `new Document()` is an XML document, as it is in a browser; the
      // HTML-document subclass and `createDocument` say otherwise.
      slots(this).html = false;
      slots(this).contentType = "application/xml";
      slots(this).url = "about:blank";
      // The first document made is the one bare constructors belong to, unless
      // a window said otherwise. A later one — from `DOMParser` or
      // `createHTMLDocument` — does not steal them.
      currentDocument ??= this;
    }
    get documentElement() { return Array.from(this._esdevChildren()).find((node) => node instanceof Element) ?? null; }
    // What a document says about itself. There is no quirks mode here — the
    // parser does not implement one — and every document is UTF-8.
    get contentType() { return slots(this).contentType; }
    get URL() { return slots(this).url; }
    get documentURI() { return slots(this).url; }
    get compatMode() { return "CSS1Compat"; }
    get characterSet() { return "UTF-8"; }
    get charset() { return "UTF-8"; }
    get inputEncoding() { return "UTF-8"; }
    // Only a document with a browsing context has a location; the window's own
    // document answers with the window's.
    get location() { return null; }
    // The ParentNode members, which a Document has as much as an element does:
    // `document.firstElementChild` is the document element.
    get children() {
      const state = slots(this);
      return state.children ??= new HTMLCollection(this, (root) => Array.from(root._esdevChildren()).filter((node) => node instanceof Element));
    }
    get firstElementChild() { return this.children.item(0); }
    get lastElementChild() { return this.children.item(this.children.length - 1); }
    get childElementCount() { return this.children.length; }
    createElement(name, options = undefined) {
      name = String(name);
      if (!isValidElementLocalName(name)) throw domError("InvalidCharacterError", `"${name}" is not a valid element name.`);
      // ASCII-lowercased in an HTML document: `createElement("DIV")` makes a
      // `div`, and so does the parser. An XML document keeps the case, and makes
      // an HTML element only if it is XHTML.
      if (isHTMLDocument(this)) name = asciiLowercase(name);
      else if (this.contentType !== "application/xhtml+xml") return new Element(name, this, null);
      return withIsValue(new (elementClass(name))(name, this), options);
    }
    createElementNS(namespaceURI, qualifiedName, options = undefined) {
      const { namespace, prefix, localName } = validateAndExtract(namespaceURI, qualifiedName, "element");
      let element;
      if (namespace === HTML_NAMESPACE) element = withIsValue(new (elementClass(localName))(localName, this), options);
      // By exact local name: SVG is case-sensitive, so `CIRCLE` is an unknown
      // element with the base interface, exactly as in a browser.
      else if (namespace === SVG_NAMESPACE) element = new (SVG_ELEMENT_CLASSES[localName] ?? SVGElement)(localName, this, namespace);
      else if (namespace === MATHML_NAMESPACE) element = new MathMLElement(localName, this, namespace);
      else element = new Element(localName, this, namespace);
      element.prefix = prefix;
      return element;
    }
    createTextNode(data) { return new Text(data, this); }
    createComment(data) { return new Comment(data, this); }
    createCDATASection(_data) {
      throw domError("NotSupportedError", "An HTML document cannot contain a CDATA section.");
    }
    createProcessingInstruction(target, data) {
      target = String(target);
      if (!/^[A-Za-z_][\w.-]*$/.test(target)) throw domError("InvalidCharacterError", "A processing instruction target must be a valid XML name.");
      if (String(data).includes("?>")) throw domError("InvalidCharacterError", "A processing instruction cannot contain '?>'.");
      return new ProcessingInstruction(target, data, this);
    }
    get doctype() { return Array.from(this._esdevChildren()).find((node) => node instanceof DocumentType) ?? null; }
    // A document has a window only when one installed itself over this getter.
    get defaultView() { return null; }
    // The first `title` element in tree order, created in the head on demand,
    // because head management libraries write it before reading it back.
    get title() {
      const element = collect(this, (node) => node.localName === "title" && node.namespaceURI === HTML_NAMESPACE)[0];
      return (element?.textContent ?? "").replace(/[\t\n\f\r ]+/g, " ").trim();
    }
    set title(value) {
      let element = collect(this, (node) => node.localName === "title" && node.namespaceURI === HTML_NAMESPACE)[0];
      if (!element) {
        const head = this.head ?? this.documentElement;
        if (!head) return;
        element = this.createElement("title");
        head._insert(element, null);
      }
      element.textContent = String(value);
    }
    get implementation() {
      this[IMPLEMENTATION] ??= new DOMImplementation(this, IMPLEMENTATION_BRAND);
      return this[IMPLEMENTATION];
    }
    createDocumentFragment() { return new DocumentFragment(this); }
    createTreeWalker(root, whatToShow = NodeFilter.SHOW_ALL, filter = null) {
      if (!(root instanceof Node)) throw new TypeError("createTreeWalker root must be a Node");
      return new TreeWalker(root, whatToShow, filter);
    }
    // Legacy, and still normative: a library that builds events this way is
    // asking for an interface by name. The modern names are answered; the
    // HTML4 aliases are refused with the constructor to use.
    createEvent(interfaceName) {
      if (typeof events.createLegacy !== "function") {
        throw domError("NotSupportedError", "This DOM has no event interfaces to create.");
      }
      return events.createLegacy(interfaceName);
    }
    createNodeIterator(root, whatToShow = NodeFilter.SHOW_ALL, filter = null) {
      if (!(root instanceof Node)) throw new TypeError("createNodeIterator root must be a Node");
      return new NodeIterator(root, whatToShow, filter);
    }
    createAttribute(name) {
      name = String(name);
      if (!isValidAttributeLocalName(name)) throw domError("InvalidCharacterError", `"${name}" is not a valid attribute name.`);
      return new Attr(isHTMLDocument(this) ? asciiLowercase(name) : name, "", this);
    }
    createAttributeNS(namespaceURI, qualifiedName) {
      const { namespace, prefix, localName } = validateAndExtract(namespaceURI, qualifiedName, "attribute");
      return new Attr(localName, "", this, namespace, prefix);
    }
    getElementsByTagName(name) { return elementsByQualifiedName(this, String(name)); }
    getElementsByTagNameNS(namespace, localName) { return elementsByNamespace(this, namespace, localName); }
    getElementsByClassName(names) {
      const expected = String(names).split(/[\t\n\f\r ]+/).filter(Boolean);
      return new HTMLCollection(this, (root) => collect(root, (element) => {
        const classes = new Set((element.getAttribute("class") ?? "").split(/[\t\n\f\r ]+/).filter(Boolean));
        return expected.every((name) => classes.has(name));
      }));
    }
    getElementById(id) {
      id = String(id);
      return collect(this, (element) => element.id === id)[0] ?? null;
    }
    adoptNode(node) {
      if (!(node instanceof Node) || node instanceof Document) throw domError("NotSupportedError", "A document cannot be adopted.");
      // A shadow root is a fragment with a host, and its host is what owns it: it
      // cannot be adopted away on its own.
      if (node instanceof ShadowRoot) throw domError("HierarchyRequestError", "A shadow root cannot be adopted.");
      if (node.parentNode) node.parentNode._remove(node);
      adoptInto(this, node);
      return node;
    }
    importNode(node, deep = false) {
      if (!(node instanceof Node) || node instanceof Document) throw domError("NotSupportedError", "A document cannot be imported.");
      const copy = node.cloneNode(deep);
      descendants(copy, (item) => { slots(item).ownerDocument = this; });
      return copy;
    }
  }

  // `[SameObject] readonly attribute DOMImplementation implementation`, and like
  // ValidityState it has no constructor of its own in Web IDL.
  const IMPLEMENTATION_BRAND = Symbol("esdev DOM implementation brand");


  // An HTML document is its own interface: a browser's `document`, a
  // `DOMParser` text/html parse and `createHTMLDocument()` are all
  // `HTMLDocument`, and code reads that through `Object.prototype.toString`
  // and `instanceof`. `new Document()` and an XML `createDocument()` stay
  // plain, as they do in a browser.
  class HTMLDocument extends Document {
    constructor() {
      super();
      slots(this).html = true;
      slots(this).contentType = "text/html";
    }
  }
  // What `createDocument` makes: an XML document, whose content type follows
  // the namespace of its root.
  class XMLDocument extends Document {}

  class DOMImplementation {
    constructor(document, brand) {
      if (brand !== IMPLEMENTATION_BRAND) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, IMPLEMENTATION, { value: document });
    }
    // Long obsolete, and specified to answer true to everything.
    hasFeature() { return true; }
    createDocumentType(name, publicId, systemId) {
      if (arguments.length < 3) throw new TypeError("createDocumentType takes a name, a public ID and a system ID.");
      name = String(name);
      if (!isValidDoctypeName(name)) throw domError("InvalidCharacterError", `"${name}" is not a valid doctype name.`);
      return new DocumentType(name, publicId, systemId, this[IMPLEMENTATION]);
    }
    // An XML document with an optional root element: no XML *parsing* is
    // involved, and this is what a sanitiser building a namespaced document for
    // comparison needs.
    createDocument(namespaceURI, qualifiedName, doctype = null) {
      if (arguments.length < 2) throw new TypeError("createDocument takes a namespace and a qualified name.");
      const document = new XMLDocument();
      const name = qualifiedName === null ? "" : String(qualifiedName);
      const element = name === "" ? null : document.createElementNS(namespaceURI, name);
      if (doctype !== null && doctype !== undefined) {
        if (!(doctype instanceof DocumentType)) throw new TypeError("createDocument doctype must be a DocumentType");
        document._preInsert(doctype, null);
      }
      if (element) document._preInsert(element, null);
      const namespace = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
      slots(document).contentType = namespace === HTML_NAMESPACE ? "application/xhtml+xml"
        : namespace === SVG_NAMESPACE ? "image/svg+xml" : "application/xml";
      return document;
    }
    createHTMLDocument(title = undefined) {
      const document = new HTMLDocument();
      document._preInsert(new DocumentType("html", "", "", document), null);
      const html = document.createElement("html");
      const head = document.createElement("head");
      const body = document.createElement("body");
      html._insertMany([head, body], null);
      document._preInsert(html, null);
      if (title !== undefined) {
        const element = document.createElement("title");
        element._insert(document.createTextNode(String(title)), null);
        head._insert(element, null);
      }
      // `head` and `body` are per-document accessors on the instance, the same
      // shape the test realm's own document gets.
      defineIdl(document, { head: { get: () => head }, body: { get: () => body } });
      return document;
    }
  }

  function showMask(node) {
    return node.nodeType === Node.ELEMENT_NODE ? NodeFilter.SHOW_ELEMENT
      : node.nodeType === Node.TEXT_NODE ? NodeFilter.SHOW_TEXT
        : node.nodeType === Node.COMMENT_NODE ? NodeFilter.SHOW_COMMENT : 0;
  }

  // Pre-order, with the reference node and `pointerBeforeReferenceNode` the
  // specification names. It is not a TreeWalker with a different surface: a
  // NodeIterator's position is *between* nodes, which is what makes going
  // forward and then back land on the same node rather than skipping one.
  class NodeIterator {
    constructor(root, whatToShow, filter) {
      defineIdl(this, {
        root: { value: root, enumerable: true },
        whatToShow: { value: Number(whatToShow) >>> 0, enumerable: true },
        filter: { value: filter ?? null, enumerable: true },
      });
      this[ITERATOR] = { reference: root, before: true };
    }
    get referenceNode() { return this[ITERATOR].reference; }
    get pointerBeforeReferenceNode() { return this[ITERATOR].before; }
    _accepts(node) {
      if (!(this.whatToShow & showMask(node))) return false;
      if (this.filter === null) return true;
      const decision = typeof this.filter === "function" ? this.filter(node) : this.filter.acceptNode(node);
      // A NodeIterator has no REJECT: a rejected node is skipped, and its
      // children are still visited.
      return decision === NodeFilter.FILTER_ACCEPT;
    }
    _step(forward) {
      const state = this[ITERATOR];
      let node = state.reference;
      let before = state.before;
      for (;;) {
        if (forward) {
          if (before) before = false;
          else {
            const next = following(node, this.root);
            if (!next) return null;
            node = next;
          }
        } else if (!before) before = true;
        else {
          const previous = preceding(node, this.root);
          if (!previous) return null;
          node = previous;
        }
        if (this._accepts(node)) {
          state.reference = node;
          state.before = before;
          return node;
        }
      }
    }
    nextNode() { return this._step(true); }
    previousNode() { return this._step(false); }
    // Long obsolete and specified to do nothing.
    detach() {}
  }

  function following(node, root) {
    if (node.firstChild) return node.firstChild;
    for (let current = node; current && current !== root; current = current.parentNode) {
      if (current.nextSibling) return current.nextSibling;
    }
    return null;
  }

  function preceding(node, root) {
    if (node === root) return null;
    let previous = node.previousSibling;
    if (!previous) return node.parentNode === root ? null : node.parentNode;
    while (previous.lastChild) previous = previous.lastChild;
    return previous;
  }

  class TreeWalker {
    constructor(root, whatToShow, filter) {
      this.root = root;
      this.whatToShow = Number(whatToShow) >>> 0;
      this.filter = filter;
      this.currentNode = root;
    }
    _result(node) {
      if (!(this.whatToShow & showMask(node))) return NodeFilter.FILTER_SKIP;
      if (this.filter === null) return NodeFilter.FILTER_ACCEPT;
      const result = typeof this.filter === "function" ? this.filter(node) : this.filter.acceptNode(node);
      return [NodeFilter.FILTER_ACCEPT, NodeFilter.FILTER_REJECT, NodeFilter.FILTER_SKIP].includes(result) ? result : NodeFilter.FILTER_SKIP;
    }
    nextNode() {
      let node = this.currentNode;
      while (node) {
        const result = node === this.currentNode ? NodeFilter.FILTER_SKIP : this._result(node);
        if (node.firstChild && result !== NodeFilter.FILTER_REJECT) node = node.firstChild;
        else {
          while (node && node !== this.root && !node.nextSibling) node = node.parentNode;
          if (!node || node === this.root) return null;
          node = node.nextSibling;
        }
        if (this._result(node) === NodeFilter.FILTER_ACCEPT) { this.currentNode = node; return node; }
      }
      return null;
    }
  }

  const ELEMENT_CLASSES = {
    button: HTMLButtonElement,
    a: HTMLAnchorElement,
    canvas: HTMLCanvasElement,
    dialog: HTMLDialogElement,
    div: HTMLDivElement,
    form: HTMLFormElement,
    input: HTMLInputElement,
    label: HTMLLabelElement,
    option: HTMLOptionElement,
    optgroup: HTMLOptGroupElement,
    progress: HTMLProgressElement,
    select: HTMLSelectElement,
    slot: HTMLSlotElement,
    textarea: HTMLTextAreaElement,
    template: HTMLTemplateElement,
    style: HTMLStyleElement,
    table: HTMLTableElement,
    thead: HTMLTableSectionElement,
    tbody: HTMLTableSectionElement,
    tfoot: HTMLTableSectionElement,
    tr: HTMLTableRowElement,
    td: HTMLTableCellElement,
    th: HTMLTableCellElement,
    caption: HTMLTableCaptionElement,
    col: HTMLTableColElement,
    colgroup: HTMLTableColElement,
    fieldset: HTMLFieldSetElement,
  };

  // The rest of the HTML interfaces, as Chrome answers them. `<script>` is an
  // `HTMLScriptElement`, `<h1>` an `HTMLHeadingElement`, `<ins>` and `<del>`
  // both `HTMLModElement` — a test that branches on the interface, or logs one,
  // reads the difference. Generated from the name map rather than declared one
  // by one, like the SVG table, because nothing about them differs but the name.
  //
  // The families above are absent here: they carry behaviour, and this adds only
  // identity.
  class HTMLMediaElement extends HTMLElement {}
  // Not an HTML element at all: a name the language does not have. `<my-thing>`
  // is an `HTMLElement` because it could still be defined; `<nonsense>` cannot.
  class HTMLUnknownElement extends HTMLElement {}

  // Seeded with the interfaces that already exist, so a name they own keeps the
  // class that carries its behaviour: generating a second, empty
  // `HTMLAnchorElement` would take `href` and `tabIndex` away from `<a>`.
  const HTML_INTERFACES = {
    HTMLMediaElement, HTMLUnknownElement, HTMLAnchorElement, HTMLButtonElement, HTMLCanvasElement,
    HTMLDialogElement, HTMLDetailsElement, HTMLDivElement, HTMLFormElement, HTMLInputElement, HTMLLabelElement,
    HTMLOptionElement, HTMLOptGroupElement, HTMLProgressElement, HTMLSelectElement, HTMLSlotElement,
    HTMLTextAreaElement, HTMLTemplateElement, HTMLStyleElement, HTMLTableElement,
    HTMLTableSectionElement, HTMLTableRowElement, HTMLTableCellElement, HTMLTableCaptionElement,
    HTMLTableColElement, HTMLFieldSetElement,
  };
  for (const [base, table] of [
    [HTMLMediaElement, { audio: "HTMLAudioElement", video: "HTMLVideoElement" }],
    [HTMLElement, {
      a: "HTMLAnchorElement", area: "HTMLAreaElement", base: "HTMLBaseElement", body: "HTMLBodyElement",
      br: "HTMLBRElement", data: "HTMLDataElement", datalist: "HTMLDataListElement",
      del: "HTMLModElement", details: "HTMLDetailsElement", dl: "HTMLDListElement",
      embed: "HTMLEmbedElement", h1: "HTMLHeadingElement", head: "HTMLHeadElement",
      hr: "HTMLHRElement", html: "HTMLHtmlElement", iframe: "HTMLIFrameElement",
      img: "HTMLImageElement", legend: "HTMLLegendElement", li: "HTMLLIElement",
      link: "HTMLLinkElement", map: "HTMLMapElement", menu: "HTMLMenuElement",
      meta: "HTMLMetaElement", meter: "HTMLMeterElement", object: "HTMLObjectElement",
      ol: "HTMLOListElement", output: "HTMLOutputElement", p: "HTMLParagraphElement",
      picture: "HTMLPictureElement", pre: "HTMLPreElement", q: "HTMLQuoteElement",
      script: "HTMLScriptElement", source: "HTMLSourceElement", span: "HTMLSpanElement",
      time: "HTMLTimeElement", title: "HTMLTitleElement", track: "HTMLTrackElement",
      ul: "HTMLUListElement",
    }],
  ]) {
    for (const [element, name] of Object.entries(table)) {
      // The computed key names the class, so `constructor.name` and the
      // `Symbol.toStringTag` taken from it read as the interface.
      HTML_INTERFACES[name] ??= { [name]: class extends base {} }[name];
      ELEMENT_CLASSES[element] ??= HTML_INTERFACES[name];
    }
  }
  // The names that share an interface with one above.
  for (const [element, name] of Object.entries({
    h2: "HTMLHeadingElement", h3: "HTMLHeadingElement", h4: "HTMLHeadingElement",
    h5: "HTMLHeadingElement", h6: "HTMLHeadingElement", ins: "HTMLModElement",
    blockquote: "HTMLQuoteElement",
  })) ELEMENT_CLASSES[element] ??= HTML_INTERFACES[name];

  // The other token lists, each [SameObject] and [PutForwards=value], with the
  // supported tokens Chrome reports.
  const LINK_TYPES = new Set(["noopener", "noreferrer", "opener"]);
  const LINK_RELATIONS = new Set(["alternate", "canonical", "dns-prefetch", "icon", "manifest", "modulepreload", "next",
    "preconnect", "prefetch", "preload", "prerender", "stylesheet", "apple-touch-icon", "apple-touch-icon-precomposed",
    "compression-dictionary"]);
  const SANDBOX_FLAGS = new Set(["allow-downloads", "allow-forms", "allow-modals", "allow-orientation-lock",
    "allow-pointer-lock", "allow-popups", "allow-popups-to-escape-sandbox", "allow-presentation", "allow-same-origin",
    "allow-scripts", "allow-storage-access-by-user-activation", "allow-top-navigation",
    "allow-top-navigation-by-user-activation"]);
  const tokenLists = new WeakMap();
  function defineTokenList(Interface, property, attribute, supported) {
    Object.defineProperty(Interface.prototype, property, {
      get() {
        let lists = tokenLists.get(this);
        if (!lists) tokenLists.set(this, lists = {});
        return lists[property] ??= new DOMTokenList(this, attribute, supported);
      },
      set(value) { this[property].value = value; },
      enumerable: true,
      configurable: true,
    });
  }
  for (const name of ["HTMLAnchorElement", "HTMLAreaElement", "HTMLFormElement"]) defineTokenList(HTML_INTERFACES[name], "relList", "rel", LINK_TYPES);
  defineTokenList(SVG_INTERFACES.SVGAElement, "relList", "rel", LINK_TYPES);
  defineTokenList(HTML_INTERFACES.HTMLLinkElement, "relList", "rel", LINK_RELATIONS);
  defineTokenList(HTML_INTERFACES.HTMLLinkElement, "sizes", "sizes", null);
  defineTokenList(HTML_INTERFACES.HTMLOutputElement, "htmlFor", "for", null);
  defineTokenList(HTML_INTERFACES.HTMLIFrameElement, "sandbox", "sandbox", SANDBOX_FLAGS);

  // A valid custom element name, which is the only kind of element that can be
  // undefined: everything else is defined by being built in.
  const CUSTOM_NAME = /^[a-z][a-z0-9._-]*-[a-z0-9._-]*$/;

  // Which class an HTML name makes. A name the language has gets its interface;
  // a hyphenated one gets `HTMLElement`, because it could still be defined; any
  // other name gets `HTMLUnknownElement`, as in a browser.
  function elementClass(name) {
    const found = ELEMENT_CLASSES[name];
    if (found !== undefined) return found;
    if (CUSTOM_NAME.test(name) || HTML_ELEMENT_NAMES.has(name)) return HTMLElement;
    return HTML_INTERFACES.HTMLUnknownElement;
  }

  function hasFailedUpgrade(element) {
    return slots(element).customFailed === true;
  }

  // The "is" value: what a customized built-in was *created* as. It is not the
  // `is` attribute — setting that attribute later customizes nothing, in a
  // browser or here — but the serializer prints it when no attribute carries
  // it, which is how a browser round-trips one.
  function withIsValue(element, options) {
    const value = options === null || options === undefined ? undefined : options.is;
    if (value !== undefined) slots(element).isValue = String(value);
    return element;
  }

  function isKnownHtmlElement(name) {
    return HTML_ELEMENT_NAMES.has(String(name).toLowerCase());
  }

  function isValueOf(element) {
    return slots(element).isValue ?? null;
  }

  // "Adopt": the node and its descendants take the new document — and HTML's
  // template adopting steps run with them, so a template's content follows into
  // the new document's template contents owner rather than staying behind in the
  // old one's. The content is not a descendant in the node tree, which is why it
  // needs saying.
  function adoptInto(document, node) {
    descendants(node, (item) => {
      slots(item).ownerDocument = document;
      const content = item[TEMPLATE_CONTENT];
      if (content === undefined) return;
      const owner = templateContentsOwner(document);
      slots(content).ownerDocument = owner;
      for (const child of Array.from(content._esdevChildren())) adoptInto(owner, child);
    });
  }

  function setCustomLookup(lookup) {
    customLookup = lookup;
  }

  function isDefined(element) {
    // A customized built-in is undefined until it upgrades, exactly like an
    // autonomous one: `<button is="my-button">` is not `:defined` while
    // `my-button` is not.
    if (element.namespaceURI === HTML_NAMESPACE && isValueOf(element) !== null) {
      return slots(element).customDefined === true;
    }
    if (element.namespaceURI !== HTML_NAMESPACE || !CUSTOM_NAME.test(element.localName)) return true;
    if (element.getRootNode()?.[INERT]) return false;
    if (slots(element).customFailed) return false;
    return slots(element).customDefined === true;
  }

  function upgradeCustom(element, constructor) {
    if (Object.getPrototypeOf(element) === constructor.prototype) return element;
    if (!(constructor.prototype instanceof HTMLElement)) throw new TypeError("Custom element constructors must extend HTMLElement");
    Object.setPrototypeOf(element, constructor.prototype);
    // "Precustomized" while the constructor runs: it is not `:defined` yet, but
    // it may attach internals — that is what a constructor does first.
    slots(element).customPrecustomized = true;
    customConstruction.push({ element, name: element.localName, document: element.ownerDocument });
    try {
      const constructed = new constructor();
      if (constructed !== element) throw new TypeError("Custom element constructor returned a different object");
    } catch (error) {
      // "Failed", which is neither custom nor uncustomized: the element is not
      // `:defined`, and nothing tries to upgrade it again.
      slots(element).customFailed = true;
      throw error;
    } finally {
      customConstruction.pop();
    }
    slots(element).customDefined = true;
    return element;
  }

  // "The list of elements with qualified name": in an HTML document an HTML
  // element matches the name lowercased and anything else matches it exactly,
  // and it is the *qualified* name that matches, so `a:b` finds `<a:b>`.
  function elementsByQualifiedName(root, name) {
    return new HTMLCollection(root, (from) => {
      if (name === "*") return collect(from, () => true);
      const html = isHTMLDocument(from.ownerDocument ?? from);
      const lower = asciiLowercase(name);
      return collect(from, (element) => {
        const qualifiedName = element.prefix === null ? element.localName : `${element.prefix}:${element.localName}`;
        return html && element.namespaceURI === HTML_NAMESPACE ? qualifiedName === lower : qualifiedName === name;
      });
    });
  }
  function elementsByNamespace(root, namespace, localName) {
    namespace = namespace == null || namespace === "" ? null : String(namespace);
    localName = String(localName);
    return new HTMLCollection(root, (from) => collect(from, (element) =>
      (namespace === "*" || element.namespaceURI === namespace) && (localName === "*" || element.localName === localName)));
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

  return { Node, staticNodeList, HTMLDocument, XMLDocument, isHTMLDocument, isValueOf, isKnownHtmlElement, ...HTML_INTERFACES, ...SVG_INTERFACES, NodeList, HTMLCollection, DOMTokenList, NodeFilter, TreeWalker, NodeIterator, Document, DocumentFragment, ShadowRoot, Element, HTMLElement, HTMLTemplateElement, HTMLSlotElement, MathMLElement, HTMLInputElement, HTMLButtonElement, HTMLDialogElement, HTMLDivElement, HTMLCanvasElement, HTMLAnchorElement, HTMLProgressElement, HTMLStyleElement, HTMLTableElement, HTMLTableSectionElement, HTMLTableRowElement, HTMLTableCellElement, HTMLTableCaptionElement, HTMLTableColElement, HTMLFormElement, HTMLLabelElement, HTMLFieldSetElement, HTMLOptGroupElement, HTMLOptionElement, HTMLSelectElement, HTMLTextAreaElement, CharacterData, Text, CDATASection, Comment, ProcessingInstruction, DocumentType, DOMImplementation, DOMStringMap, Attr, NamedNodeMap, ValidityState, ElementInternals, CustomStateSet, DOMRect, DOMRectReadOnly, VOID, HTML_NAMESPACE, SVG_NAMESPACE, MATHML_NAMESPACE, ownAttributes, setCurrentDocument, setCustomLookup, hasFailedUpgrade, isDefined, isDisabled, controlStates: customStates, customStates, controlValidity, formSubmissionValue, upgradeCustom };
}
