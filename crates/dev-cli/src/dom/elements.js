// Custom-element registration and reactions. The registry owns the reaction
// wiring while the tree retains ownership of node identity and mutation.

function validName(name) {
  return /^[a-z][a-z0-9._-]*-[a-z0-9._-]*$/.test(name);
}

export function createElements(tree) {
  const { Node, Document, Element, HTMLElement, upgradeCustom } = tree;
  // Per registry, not per module: the specification's duplicate checks are
  // "does *this* registry already have it", and a second `new
  // CustomElementRegistry()` that wrote into the document's definitions would
  // be upgrading elements it was never given.
  const state = new WeakMap();

  // The one the document was installed with. A scoped registry — the proposal
  // `attachShadow({ customElements })` belongs to — is not implemented, so its
  // definitions upgrade nothing.
  let active = null;

  function registryOf(registry) {
    let own = state.get(registry);
    if (!own) {
      own = { definitions: new Map(), names: new Map(), waiting: new Map() };
      state.set(registry, own);
    }
    return own;
  }

  function definition(element) {
    return active ? registryOf(active).definitions.get(element.localName) : undefined;
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
    // A connected callback may synchronously add a custom-element child.  It
    // receives its own insertion reaction, so walk a pre-reaction snapshot to
    // avoid invoking that child a second time while descending the parent.
    const connected = [];
    for (const child of inserted) walk(child, (element) => { if (element.isConnected) connected.push(element); });
    for (const element of connected) react(element, "connectedCallback");
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
      const own = registryOf(this);
      name = String(name);
      if (!validName(name)) throw new DOMException("Custom element names must contain a hyphen.", "SyntaxError");
      if (own.definitions.has(name)) throw new DOMException(`${name} is already defined.`, "NotSupportedError");
      if (typeof constructor !== "function") throw new TypeError("Custom element constructor must be a function");
      if (!(constructor.prototype instanceof HTMLElement)) throw new TypeError("Custom element constructors must extend HTMLElement");
      // One constructor, one name *in this registry*: a class registered twice
      // would make `getName` and a direct `new Constructor()` ambiguous.
      const taken = own.names.get(constructor);
      if (taken !== undefined) {
        throw new DOMException(`This constructor is already defined as ${taken}.`, "NotSupportedError");
      }
      own.definitions.set(name, constructor);
      own.names.set(constructor, name);
      if (this === active) {
        for (const document of documents) {
          walk(document, (element) => {
            if (element.localName !== name) return;
            const wasUpgraded = element instanceof constructor;
            upgrade(element);
            if (!wasUpgraded && element.isConnected) react(element, "connectedCallback");
          });
        }
      }
      for (const resolve of own.waiting.get(name) ?? []) resolve(constructor);
      own.waiting.delete(name);
    }
    get(name) { return registryOf(this).definitions.get(String(name)); }
    getName(constructor) { return registryOf(this).names.get(constructor) ?? null; }
    whenDefined(name) {
      const own = registryOf(this);
      name = String(name);
      if (!validName(name)) return Promise.reject(new DOMException("Custom element names must contain a hyphen.", "SyntaxError"));
      const existing = own.definitions.get(name);
      if (existing) return Promise.resolve(existing);
      return new Promise((resolve) => {
        const resolvers = own.waiting.get(name) ?? [];
        resolvers.push(resolve);
        own.waiting.set(name, resolvers);
      });
    }
    upgrade(root) { upgradeTree(root); }
  }

  const documents = new Set();
  function install(document) {
    documents.add(document);
    active = new CustomElementRegistry();
    // `new SomeElement()` with no arguments makes an element of the name the
    // class was defined under, in this document. The tree cannot know that on
    // its own, so it is told where to look.
    tree.setCustomLookup((constructor) => {
      const name = registryOf(active).names.get(constructor);
      return name === undefined ? null : { name, document };
    });
    return active;
  }

  return { CustomElementRegistry, install };
}
