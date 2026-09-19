// Boundary-point ranges for the test DOM. Layout-dependent geometry is
// intentionally absent; this covers tree positions and fragment parsing.

function rangeError(name, message) {
  throw new DOMException(message, name);
}

export function createRanges({ Node, Element, Text }, parse) {
  function childIndex(node) {
    let index = 0;
    for (let sibling = node.previousSibling; sibling; sibling = sibling.previousSibling) index += 1;
    return index;
  }

  function root(node) {
    while (node.parentNode) node = node.parentNode;
    return node;
  }

  function validate(document, node, offset) {
    if (!(node instanceof Node)) throw new TypeError("Range boundary container must be a Node");
    if (node.ownerDocument !== document && node !== document) rangeError("WrongDocumentError", "Range boundary is in another document.");
    offset = Number(offset);
    if (!Number.isInteger(offset) || offset < 0) rangeError("IndexSizeError", "Range offset must be a non-negative integer.");
    const length = node instanceof Text ? node.data.length : node.childNodes.length;
    if (offset > length) rangeError("IndexSizeError", "Range offset is past the end of its container.");
    return offset;
  }

  function comparePoints(aNode, aOffset, bNode, bOffset) {
    if (root(aNode) !== root(bNode)) rangeError("WrongDocumentError", "Range boundary points are disconnected.");
    if (aNode === bNode) return Math.sign(aOffset - bOffset);
    const aPath = [];
    const bPath = [];
    for (let node = aNode; node; node = node.parentNode) aPath.push(node);
    for (let node = bNode; node; node = node.parentNode) bPath.push(node);
    let ai = aPath.length - 1;
    let bi = bPath.length - 1;
    while (ai >= 0 && bi >= 0 && aPath[ai] === bPath[bi]) { ai -= 1; bi -= 1; }
    if (ai < 0) return aOffset <= childIndex(bPath[bi]) ? -1 : 1;
    if (bi < 0) return childIndex(aPath[ai]) < bOffset ? -1 : 1;
    return Math.sign(childIndex(aPath[ai]) - childIndex(bPath[bi]));
  }

  function textNodes(node, values = []) {
    if (node instanceof Text) values.push(node);
    for (let child = node.firstChild; child; child = child.nextSibling) textNodes(child, values);
    return values;
  }

  class Range {
    static START_TO_START = 0;
    static START_TO_END = 1;
    static END_TO_END = 2;
    static END_TO_START = 3;
    constructor(document) {
      this.document = document;
      this.startContainer = document;
      this.startOffset = 0;
      this.endContainer = document;
      this.endOffset = 0;
    }
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; }
    get commonAncestorContainer() {
      const ancestors = new Set();
      for (let node = this.startContainer; node; node = node.parentNode) ancestors.add(node);
      for (let node = this.endContainer; node; node = node.parentNode) if (ancestors.has(node)) return node;
      return null;
    }
    setStart(node, offset) {
      offset = validate(this.document, node, offset);
      this.startContainer = node; this.startOffset = offset;
      if (comparePoints(node, offset, this.endContainer, this.endOffset) > 0) { this.endContainer = node; this.endOffset = offset; }
    }
    setEnd(node, offset) {
      offset = validate(this.document, node, offset);
      this.endContainer = node; this.endOffset = offset;
      if (comparePoints(this.startContainer, this.startOffset, node, offset) > 0) { this.startContainer = node; this.startOffset = offset; }
    }
    setStartBefore(node) { if (!node.parentNode) rangeError("InvalidNodeTypeError", "Cannot set a boundary before a root node."); this.setStart(node.parentNode, childIndex(node)); }
    setStartAfter(node) { if (!node.parentNode) rangeError("InvalidNodeTypeError", "Cannot set a boundary after a root node."); this.setStart(node.parentNode, childIndex(node) + 1); }
    setEndBefore(node) { if (!node.parentNode) rangeError("InvalidNodeTypeError", "Cannot set a boundary before a root node."); this.setEnd(node.parentNode, childIndex(node)); }
    setEndAfter(node) { if (!node.parentNode) rangeError("InvalidNodeTypeError", "Cannot set a boundary after a root node."); this.setEnd(node.parentNode, childIndex(node) + 1); }
    selectNode(node) { this.setStartBefore(node); this.setEndAfter(node); }
    selectNodeContents(node) { this.setStart(node, 0); this.setEnd(node, node instanceof Text ? node.data.length : node.childNodes.length); }
    collapse(toStart = false) { if (toStart) this.setEnd(this.startContainer, this.startOffset); else this.setStart(this.endContainer, this.endOffset); }
    compareBoundaryPoints(how, sourceRange) {
      if (!(sourceRange instanceof Range)) throw new TypeError("compareBoundaryPoints expects a Range");
      if (how === Range.START_TO_START) return comparePoints(this.startContainer, this.startOffset, sourceRange.startContainer, sourceRange.startOffset);
      if (how === Range.START_TO_END) return comparePoints(this.endContainer, this.endOffset, sourceRange.startContainer, sourceRange.startOffset);
      if (how === Range.END_TO_END) return comparePoints(this.endContainer, this.endOffset, sourceRange.endContainer, sourceRange.endOffset);
      if (how === Range.END_TO_START) return comparePoints(this.startContainer, this.startOffset, sourceRange.endContainer, sourceRange.endOffset);
      rangeError("NotSupportedError", "Unknown range comparison mode.");
    }
    toString() {
      let value = "";
      for (const text of textNodes(root(this.startContainer))) {
        if (comparePoints(text, text.data.length, this.startContainer, this.startOffset) <= 0 || comparePoints(text, 0, this.endContainer, this.endOffset) >= 0) continue;
        const start = text === this.startContainer ? this.startOffset : 0;
        const end = text === this.endContainer ? this.endOffset : text.data.length;
        value += text.data.slice(start, end);
      }
      return value;
    }
    createContextualFragment(source) {
      const container = this.startContainer instanceof Element ? this.startContainer : this.startContainer.parentElement ?? this.document.body;
      return parse.parseFragment(source, container);
    }
    detach() {}
  }

  function install(document) {
    Object.defineProperty(document, "createRange", { value: () => new Range(document) });
  }

  return { Range, install };
}
