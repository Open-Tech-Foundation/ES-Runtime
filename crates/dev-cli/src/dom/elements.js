// Custom-element registration and reactions. The registry owns the reaction
// wiring while the tree retains ownership of node identity and mutation.

// IsConstructor, without calling it: `Reflect.construct` checks its new
// target before running anything.
function isConstructor(value) {
  if (typeof value !== "function") return false;
  try {
    Reflect.construct(Object, [], value);
    return true;
  } catch {
    return false;
  }
}

export function createElements(tree) {
  const { Node, Document, Element, HTMLElement, isValueOf, upgradeCustom, isValidCustomElementName: validName } = tree;
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
      own = { definitions: new Map(), names: new Map(), waiting: new Map(), extending: new Map() };
      state.set(registry, own);
    }
    return own;
  }

  // Autonomous elements are found by their own name; a customized built-in by
  // the is value it was created with, and only when that definition was
  // registered for this very element's local name — `{ extends: "button" }`
  // customizes a `button` and nothing else.
  function defined(element) {
    if (!active) return undefined;
    const own = registryOf(active);
    const is = isValueOf(element);
    if (is !== null) {
      const found = own.definitions.get(is);
      return found && own.extending.get(is) === element.localName ? found : undefined;
    }
    return own.extending.has(element.localName) ? undefined : own.definitions.get(element.localName);
  }

  function definition(element) {
    return defined(element)?.constructor;
  }

  // What the *definition* observes, read once when it was defined. A class that
  // changes its mind afterwards changes nothing, in a browser or here — and a
  // list re-read on every attribute write would call a getter Lit uses to
  // finalize a class, on every write.
  function observed(element) {
    return defined(element)?.observed ?? [];
  }

  function react(element, method, ...args) {
    // The definition's callback, not the element's: what a class was defined
    // with is what runs, so replacing a prototype method afterwards does not
    // change an element already defined.
    const callback = defined(element)?.callbacks[method];
    if (typeof callback !== "function") return;
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
  Document.prototype.createElement = function (name, options) {
    return upgrade(originalCreateElement.call(this, name, options));
  };

  const originalCreateElementNS = Document.prototype.createElementNS;
  Document.prototype.createElementNS = function (namespaceURI, qualifiedName, options) {
    return upgrade(originalCreateElementNS.call(this, namespaceURI, qualifiedName, options));
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

  // `attributeChangedCallback`, for every way an attribute can change — the
  // tree runs this at the end of each, whatever the entry point. The spec
  // enqueues it for an unchanged value too, and so does Chrome.
  tree.setAttributeReaction((element, localName, namespace, oldValue, value) => {
    if (observed(element).includes(localName)) react(element, "attributeChangedCallback", localName, oldValue, value, namespace);
  });

  // The lifecycle callbacks a definition carries, in the order a browser reads
  // them.
  const CALLBACKS = [
    "connectedCallback", "disconnectedCallback", "adoptedCallback", "attributeChangedCallback",
    "connectedMoveCallback", "formAssociatedCallback", "formDisabledCallback", "formResetCallback",
    "formStateRestoreCallback",
  ];

  // Everything about a class that the definition freezes at `define()` time:
  // its callbacks and, when it has an `attributeChangedCallback`, the
  // attributes it observes. A getter that throws here throws out of `define`,
  // which is what a browser does with it.
  function describe(constructor) {
    const callbacks = {};
    for (const name of CALLBACKS) {
      const callback = constructor.prototype?.[name];
      if (typeof callback === "function") callbacks[name] = callback;
    }
    // Only when there is something to call: a class with no
    // `attributeChangedCallback` never has its `observedAttributes` read, which
    // is observable — it is a getter, and a framework does work in it.
    let observed = [];
    if (callbacks.attributeChangedCallback) {
      const list = constructor.observedAttributes;
      if (list !== undefined && list !== null) observed = Array.from(list, String);
    }
    return { constructor, callbacks, observed };
  }

  class CustomElementRegistry {
    define(name, constructor, options = undefined) {
      const own = registryOf(this);
      name = String(name);
      // In the specification's order. Whether the class extends `HTMLElement`
      // is not asked here: constructing an element is where that fails.
      if (!isConstructor(constructor)) throw new TypeError("Custom element constructor must be a constructor");
      if (!validName(name)) throw new DOMException(`"${name}" is not a valid custom element name.`, "SyntaxError");
      if (own.definitions.has(name)) throw new DOMException(`${name} is already defined.`, "NotSupportedError");
      // `{ extends: "button" }`: the definition customizes that built-in rather
      // than naming a new element. It has to be a built-in that exists — a
      // custom name, or a name no HTML element has, would customize nothing.
      const extending = options === null || options === undefined ? undefined : options.extends;
      if (extending !== undefined) {
        const local = String(extending);
        if (validName(local)) {
          throw new DOMException(`"${local}" is a custom element name, so it cannot be extended.`, "NotSupportedError");
        }
        if (!tree.isKnownHtmlElement(local)) {
          throw new DOMException(`"${local}" is not an HTML element, so it cannot be extended.`, "NotSupportedError");
        }
      }
      // One constructor, one name *in this registry*: a class registered twice
      // would make `getName` and a direct `new Constructor()` ambiguous.
      const taken = own.names.get(constructor);
      if (taken !== undefined) {
        throw new DOMException(`This constructor is already defined as ${taken}.`, "NotSupportedError");
      }
      // Read the class before anything is recorded: a getter that throws must
      // leave the registry as it was, with the name still free.
      const record = describe(constructor);
      if (extending !== undefined) own.extending.set(name, String(extending));
      own.definitions.set(name, record);
      own.names.set(constructor, name);
      if (this === active) {
        const extended = own.extending.get(name);
        for (const document of documents) {
          walk(document, (element) => {
            if (extended === undefined ? element.localName !== name : isValueOf(element) !== name || element.localName !== extended) return;
            const wasUpgraded = element instanceof constructor;
            upgrade(element);
            if (!wasUpgraded && element.isConnected) react(element, "connectedCallback");
          });
        }
      }
      for (const resolve of own.waiting.get(name) ?? []) resolve(constructor);
      own.waiting.delete(name);
    }
    get(name) { return registryOf(this).definitions.get(String(name))?.constructor; }
    getName(constructor) { return registryOf(this).names.get(constructor) ?? null; }
    whenDefined(name) {
      const own = registryOf(this);
      name = String(name);
      if (!validName(name)) return Promise.reject(new DOMException(`"${name}" is not a valid custom element name.`, "SyntaxError"));
      const existing = own.definitions.get(name);
      if (existing) return Promise.resolve(existing.constructor);
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
      const own = registryOf(active);
      const name = own.names.get(constructor);
      if (name === undefined) return null;
      // A customized built-in constructs its built-in, carrying the is value:
      // `new MyButton()` is a `button`, not a `my-button`.
      const extended = own.extending.get(name);
      return extended === undefined ? { name, document } : { name: extended, document, is: name };
    });
    return active;
  }

  return { CustomElementRegistry, install };
}
