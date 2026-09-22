// Boundary-point ranges for the test DOM. Layout-dependent geometry is
// intentionally absent; this covers tree positions and fragment parsing.

function rangeError(name, message) {
  throw new DOMException(message, name);
}

const POINTS = Symbol("esdev DOM range boundary points");

export function createRanges({ Document, Node, Element, Text, DocumentType, Attr, DOMRect }, parse) {
  const ranges = new Set();
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

  function descendants(node, values = []) {
    for (let child = node.firstChild; child; child = child.nextSibling) {
      values.push(child);
      descendants(child, values);
    }
    return values;
  }

  function nodeStart(node) {
    return node instanceof Text ? { node, offset: 0 } : { node: node.parentNode, offset: childIndex(node) };
  }

  function nodeEnd(node) {
    return node instanceof Text ? { node, offset: node.data.length } : { node: node.parentNode, offset: childIndex(node) + 1 };
  }

  function contains(ancestor, node) {
    for (; node; node = node.parentNode) if (node === ancestor) return true;
    return false;
  }

  // The boundary points are readonly in Web IDL, so they live in a slot: the
  // tree adjustments below move them, and a test assigning to `startOffset`
  // should fail the way it would in a browser rather than quietly succeed.
  class AbstractRange {
    constructor(points) {
      if (points?.[POINTS] !== POINTS) throw new TypeError("Illegal constructor");
      Object.defineProperty(this, POINTS, { value: points });
    }
    get startContainer() { return this[POINTS].startContainer; }
    get startOffset() { return this[POINTS].startOffset; }
    get endContainer() { return this[POINTS].endContainer; }
    get endOffset() { return this[POINTS].endOffset; }
    get collapsed() { return this.startContainer === this.endContainer && this.startOffset === this.endOffset; }
    // Zeros, like an element's: the members exist so probing code runs, and
    // there is no layout behind them.
    getBoundingClientRect() { return new DOMRect(); }
    getClientRects() { return Object.freeze([]); }
  }

  // A snapshot: it records four values and never follows the tree afterwards,
  // which is the whole difference from a Range.
  class StaticRange extends AbstractRange {
    constructor(init = {}) {
      const { startContainer, startOffset, endContainer, endOffset } = init;
      for (const node of [startContainer, endContainer]) {
        if (!(node instanceof Node)) throw new TypeError("StaticRange boundary container must be a Node");
        if (DocumentType && node instanceof DocumentType || Attr && node instanceof Attr) {
          rangeError("InvalidNodeTypeError", "A StaticRange cannot start or end in a doctype or attribute node.");
        }
      }
      super({
        [POINTS]: POINTS,
        startContainer,
        startOffset: Number(startOffset ?? 0),
        endContainer,
        endOffset: Number(endOffset ?? 0),
      });
    }
  }

  class Range extends AbstractRange {
    static START_TO_START = 0;
    static START_TO_END = 1;
    static END_TO_END = 2;
    static END_TO_START = 3;
    constructor(document) {
      super({ [POINTS]: POINTS, startContainer: document, startOffset: 0, endContainer: document, endOffset: 0 });
      this.document = document;
      ranges.add(this);
    }
    get commonAncestorContainer() {
      const ancestors = new Set();
      for (let node = this.startContainer; node; node = node.parentNode) ancestors.add(node);
      for (let node = this.endContainer; node; node = node.parentNode) if (ancestors.has(node)) return node;
      return null;
    }
    setStart(node, offset) {
      offset = validate(this.document, node, offset);
      const points = this[POINTS];
      points.startContainer = node; points.startOffset = offset;
      if (comparePoints(node, offset, this.endContainer, this.endOffset) > 0) { points.endContainer = node; points.endOffset = offset; }
    }
    setEnd(node, offset) {
      offset = validate(this.document, node, offset);
      const points = this[POINTS];
      points.endContainer = node; points.endOffset = offset;
      if (comparePoints(this.startContainer, this.startOffset, node, offset) > 0) { points.startContainer = node; points.startOffset = offset; }
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
    insertNode(node) {
      if (!(node instanceof Node)) throw new TypeError("insertNode expects a Node");
      const container = this.startContainer;
      if (container instanceof Text) {
        const parent = container.parentNode;
        if (!parent) rangeError("HierarchyRequestError", "Cannot insert beside a detached text node.");
        const tail = container.data.slice(this.startOffset);
        container.data = container.data.slice(0, this.startOffset);
        const reference = tail === "" ? container.nextSibling : this.document.createTextNode(tail);
        if (tail !== "") parent.insertBefore(reference, container.nextSibling);
        parent.insertBefore(node, reference);
        return;
      }
      container.insertBefore(node, container.childNodes.item(this.startOffset));
    }
    deleteContents() {
      if (this.collapsed) return;
      if (this.startContainer === this.endContainer && this.startContainer instanceof Text) {
        const text = this.startContainer;
        text.data = text.data.slice(0, this.startOffset) + text.data.slice(this.endOffset);
        this.setEnd(text, this.startOffset);
        return;
      }
      const start = { node: this.startContainer, offset: this.startOffset };
      const end = { node: this.endContainer, offset: this.endOffset };
      if (start.node instanceof Text) start.node.data = start.node.data.slice(0, start.offset);
      if (end.node instanceof Text) end.node.data = end.node.data.slice(end.offset);
      const selected = [];
      for (const node of descendants(root(start.node))) {
        if (!node.parentNode) continue;
        const index = childIndex(node);
        if (comparePoints(node.parentNode, index, start.node, start.offset) >= 0
          && comparePoints(node.parentNode, index + 1, end.node, end.offset) <= 0
          && !selected.some((ancestor) => {
            for (let current = node.parentNode; current; current = current.parentNode) if (current === ancestor) return true;
            return false;
          })) selected.push(node);
      }
      for (const node of selected) node.remove();
      this.setEnd(this.startContainer, this.startOffset);
    }
    cloneContents() {
      const fragment = this.document.createDocumentFragment();
      const intersects = (node) => {
        const start = nodeStart(node);
        const end = nodeEnd(node);
        return comparePoints(end.node, end.offset, this.startContainer, this.startOffset) > 0
          && comparePoints(start.node, start.offset, this.endContainer, this.endOffset) < 0;
      };
      const contained = (node) => {
        const start = nodeStart(node);
        const end = nodeEnd(node);
        return comparePoints(start.node, start.offset, this.startContainer, this.startOffset) >= 0
          && comparePoints(end.node, end.offset, this.endContainer, this.endOffset) <= 0;
      };
      const clone = (node) => {
        if (!intersects(node)) return null;
        if (contained(node)) return node.cloneNode(true);
        if (node instanceof Text) {
          const start = node === this.startContainer ? this.startOffset : 0;
          const end = node === this.endContainer ? this.endOffset : node.data.length;
          return this.document.createTextNode(node.data.slice(start, end));
        }
        const copy = node.cloneNode(false);
        for (let child = node.firstChild; child; child = child.nextSibling) {
          const selected = clone(child);
          if (selected) copy.appendChild(selected);
        }
        return copy.hasChildNodes() ? copy : null;
      };
      if (this.startContainer === this.endContainer && this.startContainer instanceof Text) {
        fragment.appendChild(this.document.createTextNode(this.startContainer.data.slice(this.startOffset, this.endOffset)));
        return fragment;
      }
      const common = this.commonAncestorContainer;
      for (let child = common.firstChild; child; child = child.nextSibling) {
        const selected = clone(child);
        if (selected) fragment.appendChild(selected);
      }
      return fragment;
    }
    extractContents() {
      const fragment = this.cloneContents();
      this.deleteContents();
      return fragment;
    }
    surroundContents(node) {
      if (!(node instanceof Element)) throw new TypeError("surroundContents expects an Element");
      for (const candidate of descendants(this.commonAncestorContainer)) {
        if (candidate instanceof Text || candidate === this.commonAncestorContainer || !candidate.parentNode) continue;
        const start = nodeStart(candidate);
        const end = nodeEnd(candidate);
        const intersects = comparePoints(end.node, end.offset, this.startContainer, this.startOffset) > 0
          && comparePoints(start.node, start.offset, this.endContainer, this.endOffset) < 0;
        const contained = comparePoints(start.node, start.offset, this.startContainer, this.startOffset) >= 0
          && comparePoints(end.node, end.offset, this.endContainer, this.endOffset) <= 0;
        if (intersects && !contained) rangeError("InvalidStateError", "Range partially contains a non-text node.");
      }
      const fragment = this.extractContents();
      this.insertNode(node);
      node.appendChild(fragment);
      this.selectNode(node);
    }
    createContextualFragment(source) {
      const container = this.startContainer instanceof Element ? this.startContainer : this.startContainer.parentElement ?? this.document.body;
      return parse.parseFragment(source, container);
    }
    detach() { ranges.delete(this); }
  }

  // Installed on the prototype, not on one document: a document from
  // `DOMParser` or `createHTMLDocument` is a document, and `createRange` on it
  // used to be undefined.
  function install() {
    Object.defineProperty(Document.prototype, "createRange", {
      value() { return new Range(this); },
      writable: true,
      configurable: true,
    });
    Object.defineProperty(Document.prototype, "_adjustRanges", {
      get() {
        const document = this;
        return {
        insert(parent, index) {
          for (const range of ranges) {
            if (range.document !== document) continue;
            if (range.startContainer === parent && range.startOffset > index) range[POINTS].startOffset += 1;
            if (range.endContainer === parent && range.endOffset > index) range[POINTS].endOffset += 1;
          }
        },
        remove(parent, node, index) {
          for (const range of ranges) {
            if (range.document !== document) continue;
            for (const boundary of ["start", "end"]) {
              const container = range[`${boundary}Container`];
              if (contains(node, container)) {
                range[POINTS][`${boundary}Container`] = parent;
                range[POINTS][`${boundary}Offset`] = index;
              } else if (container === parent && range[`${boundary}Offset`] > index) {
                range[POINTS][`${boundary}Offset`] -= 1;
              }
            }
          }
        },
        characterData(node, _oldLength, newLength) {
          for (const range of ranges) {
            if (range.document !== document) continue;
            if (range.startContainer === node) range[POINTS].startOffset = Math.min(range.startOffset, newLength);
            if (range.endContainer === node) range[POINTS].endOffset = Math.min(range.endOffset, newLength);
          }
        },
        };
      },
      configurable: true,
    });
  }

  return { AbstractRange, Range, StaticRange, install };
}
