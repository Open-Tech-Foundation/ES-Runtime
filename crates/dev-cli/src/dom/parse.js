// Decodes the strict parser's flat records and serializes the same JS tree.
// The parser callback is supplied by the esdev-only bridge, keeping this file
// pure JS and directly testable before the --dom runner integration lands.

export function createParsing(tree, parseRecords) {
  const { Node, DocumentFragment, ShadowRoot, Element, HTMLTemplateElement, Text, Comment, VOID, HTML_NAMESPACE, SVG_NAMESPACE, MATHML_NAMESPACE } = tree;

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
      target.appendChild(node);
      nodes.push(node);
    }
    return nodes.filter((_, index) => records[index][1] === -1);
  }

  function escapeText(value) { return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;"); }
  function escapeAttribute(value) { return String(value).replaceAll("&", "&amp;").replaceAll('"', "&quot;").replaceAll("\u00a0", "&nbsp;"); }
  function serialize(node) {
    if (node instanceof Text) return escapeText(node.data);
    if (node instanceof Comment) return `<!--${node.data}-->`;
    if (node instanceof DocumentFragment || node instanceof ShadowRoot || node.nodeType === Node.DOCUMENT_NODE) return Array.from(node.childNodes, serialize).join("");
    if (!(node instanceof Element)) throw new TypeError("Cannot serialize this node type");
    const attributes = Array.from(node.attributes, (attribute) => ` ${attribute.name}="${escapeAttribute(attribute.value)}"`).join("");
    if (VOID.has(node.localName)) return `<${node.localName}${attributes}>`;
    const raw = node.localName === "script" || node.localName === "style";
    const contents = node instanceof HTMLTemplateElement ? node.content.childNodes : node.childNodes;
    const children = Array.from(contents, (child) => raw && child instanceof Text ? child.data : serialize(child)).join("");
    return `<${node.localName}${attributes}>${children}</${node.localName}>`;
  }

  function parseFragment(source, context) {
    const fragment = (context.ownerDocument ?? context).createDocumentFragment();
    decode(parseRecords(String(source), context.localName ?? null), fragment);
    return fragment;
  }

  function install() {
    for (const Class of [Element, ShadowRoot]) Object.defineProperties(Class.prototype, {
      innerHTML: {
        get() { return Array.from(this.childNodes, serialize).join(""); },
        set(source) { this.replaceChildren(parseFragment(source, this)); },
      },
      outerHTML: {
        get() { return serialize(this); },
        set(source) {
          if (!this.parentNode) return;
          this.parentNode.replaceChild(parseFragment(source, this), this);
        },
      },
    });
    Object.defineProperties(HTMLTemplateElement.prototype, {
      innerHTML: {
        get() { return Array.from(this.content.childNodes, serialize).join(""); },
        set(source) { this.content.replaceChildren(parseFragment(source, this)); },
      },
    });
    Node.prototype.replaceChildren = function (...nodes) {
      this.textContent = "";
      this.append(...nodes);
    };
  }

  return { decode, serialize, parseFragment, install };
}
