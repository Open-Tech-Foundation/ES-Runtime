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
    if (!definition(element) || typeof callback !== "function") return;
    try {
      callback.apply(element, args);
    } catch (error) {
      // A reaction runs inside a tree mutation. An exception there must not
      // unwind the mutation, so it is reported the way the constructor's is.
      report(error);
    }
  }

  // Shadow-including: a custom element inside a shadow root is as much in the
  // tree as one beside it, and `define` has to find it.
  function walk(root, visitor) {
    if (root instanceof Element) {
      visitor(root);
      const shadow = root._esdevShadowRoot?.();
      if (shadow) walk(shadow, visitor);
    }
    for (let child = root.firstChild; child; child = child.nextSibling) walk(child, visitor);
  }

  function upgrade(element) {
    const constructor = definition(element);
    if (!constructor || element instanceof constructor) return element;
    if (tree.hasFailedUpgrade(element)) return element;
    try {
      upgradeCustom(element, constructor);
    } catch (error) {
      // Reported, not rethrown: "create an element" and "upgrade an element"
      // both report a constructor's exception and carry on with an element that
      // failed to upgrade, rather than making the caller's `createElement`
      // throw something it cannot handle.
      report(error);
      return element;
    }
    for (const name of observed(element)) {
      const value = element.getAttribute(name);
      if (value !== null) react(element, "attributeChangedCallback", name, null, value);
    }
    return element;
  }

  function report(error) {
    if (typeof globalThis.reportError === "function") globalThis.reportError(error);
    else console.error(error);
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

  // A move keeps the node's state and its connection, and still reports the
  // reactions: Chrome fires `disconnectedCallback` and then `connectedCallback`
  // around a `moveBefore`, and a component that counts them would otherwise be
  // wrong about how many times it was connected.
  const originalMoveBefore = Node.prototype.moveBefore;
  Node.prototype.moveBefore = function (node, child) {
    const connected = node?.isConnected === true;
    const result = originalMoveBefore.call(this, node, child);
    if (connected) {
      walk(node, (element) => react(element, "disconnectedCallback"));
      walk(node, (element) => react(element, "connectedCallback"));
    }
    return result;
  };

  const originalAdoptNode = Document.prototype.adoptNode;
  Document.prototype.adoptNode = function (node) {
    const oldDocument = node.ownerDocument;
    const adopted = originalAdoptNode.call(this, node);
    // Only when the document actually changed: adopting a node into the
    // document it already belongs to moves nothing, and the specification's
    // "adopt" steps run the callback for a *change* of node document.
    if (oldDocument !== this) {
      walk(adopted, (element) => react(element, "adoptedCallback", oldDocument, this));
    }
    return adopted;
  };

  const originalSetAttribute = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {
    name = String(name);
    const oldValue = this.getAttribute(name);
    originalSetAttribute.call(this, name, value);
    const newValue = this.getAttribute(name);
    if (oldValue !== newValue && observed(this).includes(name)) react(this, "attributeChangedCallback", name, oldValue, newValue, null);
  };

  // The namespaced pair, which reports the *local* name and the namespace: an
  // `observedAttributes` entry names an attribute, not a qualified name.
  const originalSetAttributeNS = Element.prototype.setAttributeNS;
  Element.prototype.setAttributeNS = function (namespaceURI, qualifiedName, value) {
    const namespace = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
    const localName = String(qualifiedName).split(":").pop();
    const oldValue = this.getAttributeNS(namespace, localName);
    originalSetAttributeNS.call(this, namespaceURI, qualifiedName, value);
    const newValue = this.getAttributeNS(namespace, localName);
    if (oldValue !== newValue && observed(this).includes(localName)) {
      react(this, "attributeChangedCallback", localName, oldValue, newValue, namespace);
    }
  };

  const originalRemoveAttributeNS = Element.prototype.removeAttributeNS;
  Element.prototype.removeAttributeNS = function (namespaceURI, localName) {
    const namespace = namespaceURI == null || namespaceURI === "" ? null : String(namespaceURI);
    const name = String(localName);
    const oldValue = this.getAttributeNS(namespace, name);
    originalRemoveAttributeNS.call(this, namespaceURI, localName);
    if (oldValue !== null && observed(this).includes(name)) {
      react(this, "attributeChangedCallback", name, oldValue, null, namespace);
    }
  };

  const originalRemoveAttribute = Element.prototype.removeAttribute;
  Element.prototype.removeAttribute = function (name) {
    name = String(name);
    const oldValue = this.getAttribute(name);
    originalRemoveAttribute.call(this, name);
    if (oldValue !== null && observed(this).includes(name)) react(this, "attributeChangedCallback", name, oldValue, null, null);
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
