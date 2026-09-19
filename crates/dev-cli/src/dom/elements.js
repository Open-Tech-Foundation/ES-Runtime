// Custom-element registration and reactions. The registry owns the reaction
// wiring while the tree retains ownership of node identity and mutation.

function validName(name) {
  return /^[a-z][a-z0-9._-]*-[a-z0-9._-]*$/.test(name);
}

export function createElements(tree) {
  const { Node, Document, Element, HTMLElement, upgradeCustom } = tree;
  const definitions = new Map();
  const waiting = new Map();

  function definition(element) {
    return definitions.get(element.localName);
  }

  function observed(element) {
    const attributes = element.constructor.observedAttributes;
    return Array.isArray(attributes) ? attributes.map(String) : [];
  }

  function react(element, method, ...args) {
    const callback = element[method];
    if (definition(element) && typeof callback === "function") callback.apply(element, args);
  }

  function walk(root, visitor) {
    if (root instanceof Element) visitor(root);
    for (let child = root.firstChild; child; child = child.nextSibling) walk(child, visitor);
  }

  function upgrade(element) {
    const constructor = definition(element);
    if (!constructor || element instanceof constructor) return element;
    upgradeCustom(element, constructor);
    for (const name of observed(element)) {
      const value = element.getAttribute(name);
      if (value !== null) react(element, "attributeChangedCallback", name, null, value);
    }
    return element;
  }

  function upgradeTree(root) {
    walk(root, upgrade);
  }

  const originalCreateElement = Document.prototype.createElement;
  Document.prototype.createElement = function (name) {
    return upgrade(originalCreateElement.call(this, name));
  };

  const originalInsert = Node.prototype._insert;
  Node.prototype._insert = function (node, before) {
    const inserted = node.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? Array.from(node.childNodes) : [node];
    for (const child of inserted) upgradeTree(child);
    const result = originalInsert.call(this, node, before);
    for (const child of inserted) walk(child, (element) => { if (element.isConnected) react(element, "connectedCallback"); });
    return result;
  };

  const originalRemove = Node.prototype._remove;
  Node.prototype._remove = function (child) {
    const connected = child.isConnected;
    const result = originalRemove.call(this, child);
    if (connected) walk(child, (element) => react(element, "disconnectedCallback"));
    return result;
  };

  const originalAdoptNode = Document.prototype.adoptNode;
  Document.prototype.adoptNode = function (node) {
    const oldDocument = node.ownerDocument;
    const adopted = originalAdoptNode.call(this, node);
    walk(adopted, (element) => react(element, "adoptedCallback", oldDocument, this));
    return adopted;
  };

  const originalSetAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    name = String(name);
    const oldValue = this.getAttribute(name);
    originalSetAttribute.call(this, name, value);
    const newValue = this.getAttribute(name);
    if (oldValue !== newValue && observed(this).includes(name)) react(this, "attributeChangedCallback", name, oldValue, newValue);
  };

  const originalRemoveAttribute = Element.prototype.removeAttribute;
  Element.prototype.removeAttribute = function (name) {
    name = String(name);
    const oldValue = this.getAttribute(name);
    originalRemoveAttribute.call(this, name);
    if (oldValue !== null && observed(this).includes(name)) react(this, "attributeChangedCallback", name, oldValue, null);
  };

  class CustomElementRegistry {
    define(name, constructor) {
      name = String(name);
      if (!validName(name)) throw new DOMException("Custom element names must contain a hyphen.", "SyntaxError");
      if (definitions.has(name)) throw new DOMException(`${name} is already defined.`, "NotSupportedError");
      if (typeof constructor !== "function") throw new TypeError("Custom element constructor must be a function");
      if (!(constructor.prototype instanceof HTMLElement)) throw new TypeError("Custom element constructors must extend HTMLElement");
      definitions.set(name, constructor);
      for (const document of documents) {
        walk(document, (element) => {
          const wasUpgraded = element instanceof constructor;
          upgrade(element);
          if (!wasUpgraded && element.isConnected) react(element, "connectedCallback");
        });
      }
      for (const resolve of waiting.get(name) ?? []) resolve(constructor);
      waiting.delete(name);
    }
    get(name) { return definitions.get(String(name)); }
    whenDefined(name) {
      name = String(name);
      if (!validName(name)) return Promise.reject(new DOMException("Custom element names must contain a hyphen.", "SyntaxError"));
      const existing = definitions.get(name);
      if (existing) return Promise.resolve(existing);
      return new Promise((resolve) => {
        const resolvers = waiting.get(name) ?? [];
        resolvers.push(resolve);
        waiting.set(name, resolvers);
      });
    }
    upgrade(root) { upgradeTree(root); }
  }

  const documents = new Set();
  function install(document) {
    documents.add(document);
    return new CustomElementRegistry();
  }

  return { CustomElementRegistry, install };
}
