// Decodes the strict parser's flat records and serializes the same JS tree.
// The parser callback is supplied by the esdev-only bridge, keeping this file
// pure JS and directly testable before the --dom runner integration lands.

export function createParsing(tree, parseRecords, parseDocumentRecords = null) {
  const { Node, Document, DocumentFragment, ShadowRoot, Element, HTMLTemplateElement, Text, Comment, VOID, HTML_NAMESPACE, SVG_NAMESPACE, MATHML_NAMESPACE, ownAttributes } = tree;

  // The elements the HTML parser puts in the head when no explicit `head` was
  // written. Everything else a bare document names belongs to the body.
  const HEAD_ONLY = new Set(["base", "link", "meta", "noscript", "script", "style", "template", "title"]);

  // XML is outside this DOM's scope, so the types are recognised and then
  // refused by name rather than silently parsed as HTML.
  const XML_TYPES = new Set(["text/xml", "application/xml", "application/xhtml+xml", "image/svg+xml"]);

  const SVG_ELEMENT_NAMES = new Map([
    ["clippath", "clipPath"],
    ["foreignobject", "foreignObject"],
    ["lineargradient", "linearGradient"],
  ]);

  function elementNamespace(name, parent) {
    const namespace = parent instanceof Element ? parent.namespaceURI : HTML_NAMESPACE;
    if (namespace === SVG_NAMESPACE) return parent.localName === "foreignObject" ? HTML_NAMESPACE : SVG_NAMESPACE;
    if (namespace === MATHML_NAMESPACE) return MATHML_NAMESPACE;
    if (name.toLowerCase() === "svg" || name === "svg:svg") return SVG_NAMESPACE;
    if (name.toLowerCase() === "math") return MATHML_NAMESPACE;
    return HTML_NAMESPACE;
  }

  function decode(records, parent) {
    if (!Array.isArray(records)) throw new TypeError("DOM parser records must be an array");
    const document = parent.ownerDocument ?? parent;
    const nodes = [];
    for (let index = 0; index < records.length; index += 1) {
      const record = records[index];
      if (!Array.isArray(record) || record.length !== 5) throw new TypeError("Invalid DOM parser record");
      const [kind, parentIndex, name, attributes, text] = record;
      let node;
      if (kind === Node.ELEMENT_NODE) {
        const target = parentIndex === -1 ? parent : nodes[parentIndex];
        const context = target instanceof Element ? target : parent;
        const namespace = elementNamespace(name, context);
        const qualifiedName = namespace === SVG_NAMESPACE ? SVG_ELEMENT_NAMES.get(name.toLowerCase()) ?? name : name;
        // HTML parser-created elements do not go through the public
        // createElementNS hook; retain that observable construction path.
        node = namespace === HTML_NAMESPACE
          ? document.createElement(qualifiedName)
          : document.createElementNS(namespace, qualifiedName);
        if (!Array.isArray(attributes)) throw new TypeError("Element attributes must be an array");
        for (const attribute of attributes) {
          if (!Array.isArray(attribute) || attribute.length !== 2) throw new TypeError("Invalid DOM parser attribute");
          node.setAttribute(attribute[0], attribute[1]);
        }
      } else if (kind === Node.TEXT_NODE) node = document.createTextNode(text);
      else if (kind === Node.COMMENT_NODE) node = document.createComment(text);
      else throw new TypeError(`Unsupported DOM parser node kind: ${kind}`);
      let target = parentIndex === -1 ? parent : nodes[parentIndex];
      if (target instanceof HTMLTemplateElement) target = target.content;
      if (!target || parentIndex >= index) throw new TypeError("DOM parser parent index is invalid");
      // HTML parser tree construction is not observable at all: it is not a
      // sequence of `appendChild` calls, so it goes through the internal
      // insertion — which still runs the normal reactions — rather than through
      // any prototype method a patch or a spy could have replaced.
      target._preInsert(node, null);
      nodes.push(node);
    }
    return nodes.filter((_, index) => records[index][1] === -1);
  }

  function escapeText(value) { return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;"); }
  function escapeAttribute(value) { return String(value).replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("\u00a0", "&nbsp;"); }
  // `options` is `getHTML`'s dictionary and is empty for `innerHTML`, which is
  // specified never to serialize a shadow root.
  function shadowFor(element, options) {
    // The element's own root, open or closed: `serializableShadowRoots` and a
    // root named in `shadowRoots` are exactly how a closed one is serialized,
    // and `element.shadowRoot` cannot see it.
    const shadow = element._esdevShadowRoot?.() ?? element.shadowRoot;
    if (!shadow) return null;
    if (Array.from(options.shadowRoots ?? []).includes(shadow)) return shadow;
    return options.serializableShadowRoots && shadow.serializable ? shadow : null;
  }

  function serializeShadow(shadow, options) {
    const flags = [
      ` shadowrootmode="${shadow.mode}"`,
      shadow.delegatesFocus ? " shadowrootdelegatesfocus=\"\"" : "",
      shadow.clonable ? " shadowrootclonable=\"\"" : "",
      shadow.serializable ? " shadowrootserializable=\"\"" : "",
      shadow.slotAssignment === "manual" ? ' shadowrootslotassignment="manual"' : "",
    ].join("");
    return `<template${flags}>${Array.from(shadow.childNodes, (child) => serialize(child, options)).join("")}</template>`;
  }

  function serialize(node, options = {}) {
    if (node instanceof Text) return escapeText(node.data);
    if (node instanceof Comment) return `<!--${node.data}-->`;
    if (node instanceof DocumentFragment || node instanceof ShadowRoot || node.nodeType === Node.DOCUMENT_NODE) return Array.from(node.childNodes, (child) => serialize(child, options)).join("");
    if (!(node instanceof Element)) throw new TypeError("Cannot serialize this node type");
    // The element's own map, not the public accessor: serializing is an
    // internal read, and a test spying on `attributes` should not see it.
    const attributes = Array.from(ownAttributes(node), (attribute) => ` ${attribute.name}="${escapeAttribute(attribute.value)}"`).join("");
    if (VOID.has(node.localName)) return `<${node.localName}${attributes}>`;
    const raw = node.localName === "script" || node.localName === "style";
    const contents = node instanceof HTMLTemplateElement ? node.content.childNodes : node.childNodes;
    const shadow = node instanceof Element ? shadowFor(node, options) : null;
    const children = (shadow ? serializeShadow(shadow, options) : "")
      + Array.from(contents, (child) => raw && child instanceof Text ? child.data : serialize(child, options)).join("");
    return `<${node.localName}${attributes}>${children}</${node.localName}>`;
  }

  // `<template shadowrootmode>` is inert markup everywhere except the two entry
  // points that are specified to process it: document parsing and
  // `setHTMLUnsafe`. `innerHTML` deliberately leaves the template alone, which
  // is why this is a separate pass over an already-parsed fragment rather than
  // part of the decoder.
  function attachDeclarativeShadowRoots(root) {
    for (const template of Array.from(root.childNodes)) {
      if (!(template instanceof HTMLTemplateElement)) {
        if (template instanceof Element) attachDeclarativeShadowRoots(template);
        continue;
      }
      const mode = (template.getAttribute("shadowrootmode") ?? "").toLowerCase();
      const host = template.parentNode;
      if (mode !== "open" && mode !== "closed" || !(host instanceof Element)) {
        attachDeclarativeShadowRoots(template.content);
        continue;
      }
      // A second declarative root for the same host is dropped, template and
      // all: there is nowhere to put it and nothing to report it to.
      let shadow = null;
      try {
        shadow = host.attachShadow({
          mode,
          delegatesFocus: template.hasAttribute("shadowrootdelegatesfocus"),
          clonable: template.hasAttribute("shadowrootclonable"),
          serializable: template.hasAttribute("shadowrootserializable"),
          slotAssignment: template.getAttribute("shadowrootslotassignment") ?? "named",
        });
      } catch {
        template.parentNode._remove(template);
        continue;
      }
      attachDeclarativeShadowRoots(template.content);
      shadow._insertMany(Array.from(template.content.childNodes), null);
      template.parentNode._remove(template);
    }
  }

  // A document parse, not a fragment: the doctype is legal here and nowhere
  // else. The strict parser does not synthesize a root, so a source that names
  // `html` supplies its own and anything else is distributed into a synthesized
  // `html`/`head`/`body` the way the HTML parser would have.
  function parseDocument(source) {
    if (!parseDocumentRecords) throw new DOMException("This DOM has no document parser attached.", "NotSupportedError");
    const [hasDoctype, records] = parseDocumentRecords(String(source));
    const document = new Document();
    if (hasDoctype) document._preInsert(document.implementation.createDocumentType("html"), null);
    const holder = document.createDocumentFragment();
    decode(records, holder);
    const roots = Array.from(holder.childNodes);
    const supplied = roots.find((node) => node instanceof Element && node.localName === "html" && node.namespaceURI === HTML_NAMESPACE);
    const html = supplied ?? document.createElement("html");
    let head = Array.from(html._esdevChildren()).find((node) => node.localName === "head");
    let body = Array.from(html._esdevChildren()).find((node) => node.localName === "body");
    if (!head) html._preInsert(head = document.createElement("head"), html.firstChild);
    if (!body) html._preInsert(body = document.createElement("body"), null);
    if (!supplied) {
      for (const node of roots) {
        const intoHead = node instanceof Element && HEAD_ONLY.has(node.localName) && node.namespaceURI === HTML_NAMESPACE;
        (intoHead ? head : body)._preInsert(node, null);
      }
    }
    document._preInsert(html, null);
    Object.defineProperties(document, { head: { get: () => head }, body: { get: () => body } });
    return document;
  }

  class DOMParser {
    parseFromString(source, type) {
      type = String(type);
      if (type === "text/html") return parseDocument(source);
      if (XML_TYPES.has(type)) throw new DOMException(`${type} is outside this DOM's scope, which parses text/html only.`, "NotSupportedError");
      throw new TypeError(`'${type}' is not a supported DOMParser type.`);
    }
  }

  function parseFragment(source, context) {
    const fragment = (context.ownerDocument ?? context).createDocumentFragment();
    decode(parseRecords(String(source), context.localName ?? null), fragment);
    return fragment;
  }

  function install() {
    // A whole document from markup, declarative shadow roots included — the
    // "unsafe" in the name is about trusting the markup, not about the parser.
    Object.defineProperty(Document, "parseHTMLUnsafe", {
      value(html) {
        const document = parseDocument(html);
        attachDeclarativeShadowRoots(document);
        return document;
      },
      writable: true,
      configurable: true,
    });
    for (const Class of [Element, ShadowRoot]) Object.defineProperties(Class.prototype, {
      innerHTML: {
        get() { return Array.from(this.childNodes, serialize).join(""); },
        set(source) { this._replaceAll(parseFragment(source, this)); },
      },
      outerHTML: {
        get() { return serialize(this); },
        set(source) {
          if (!this.parentNode) return;
          const parent = this.parentNode;
          const reference = this.nextSibling;
          parent._remove(this);
          parent._insert(parseFragment(source, this), reference);
        },
      },
    });
    for (const Class of [Element, ShadowRoot]) Object.defineProperties(Class.prototype, {
      setHTMLUnsafe: { value(source) {
        const fragment = parseFragment(source, this);
        attachDeclarativeShadowRoots(fragment);
        this._replaceAll(fragment);
      }, writable: true, configurable: true },
      getHTML: { value(options = {}) {
        return Array.from(this.childNodes, (child) => serialize(child, options)).join("");
      }, writable: true, configurable: true },
    });
    Object.defineProperties(HTMLTemplateElement.prototype, {
      setHTMLUnsafe: { value(source) {
        const fragment = parseFragment(source, this);
        attachDeclarativeShadowRoots(fragment);
        this.content._replaceAll(fragment);
      }, writable: true, configurable: true },
      getHTML: { value(options = {}) {
        return Array.from(this.content.childNodes, (child) => serialize(child, options)).join("");
      }, writable: true, configurable: true },
    });
    Object.defineProperties(Element.prototype, {
      insertAdjacentHTML: { value(where, source) {
        // The parse context is where the markup lands, not the element the call
        // was made on: `beforebegin` markup is parsed as a child of the parent.
        const { parent, reference, context } = this._adjacentPosition(where);
        parent._preInsert(parseFragment(source, context), reference);
      }, writable: true, configurable: true },
    });
    Object.defineProperties(HTMLTemplateElement.prototype, {
      innerHTML: {
        get() { return Array.from(this.content.childNodes, serialize).join(""); },
        set(source) { this.content._replaceAll(parseFragment(source, this)); },
      },
    });
  }

  return { decode, serialize, parseFragment, parseDocument, DOMParser, install };
}
