// Boundary-point ranges for the test DOM. Layout-dependent geometry is
// intentionally absent; this covers tree positions and fragment parsing.

function rangeError(name, message) {
  throw new DOMException(message, name);
}

const POINTS = Symbol("esdev DOM range boundary points");

// Web IDL constants: on the interface and its prototype, so `node.ELEMENT_NODE`
// reads as `Node.ELEMENT_NODE` does, and neither writable nor configurable.
function defineConstants(Interface, names) {
  for (const name of names) {
    const value = Interface[name];
    for (const target of [Interface, Interface.prototype]) {
      Object.defineProperty(target, name, { value, writable: false, enumerable: true, configurable: false });
    }
  }
}

export function createRanges({ Document, Node, Element, Text, CharacterData, DocumentType, Attr, DOMRect }, parse) {
  // Live ranges, indexed by the nodes their boundaries are in. A mutation
  // moves only the ranges anchored where it happened — an editor's thousands
  // of ranges are not walked on every keystroke — and a document with none
  // does no range work at all. The index is keyed weakly by node, and a range
  // that is collected leaves it.
  const byNode = new WeakMap();
  let tracked = 0;
  const collected = new FinalizationRegistry((points) => {
    tracked -= 1;
    byNode.get(points.startContainer)?.delete(points);
    byNode.get(points.endContainer)?.delete(points);
  });
  function anchor(node, points) {
    let set = byNode.get(node);
    if (!set) byNode.set(node, set = new Set());
    set.add(points);
  }
  function release(node, points) {
    if (points.startContainer !== node && points.endContainer !== node) byNode.get(node)?.delete(points);
  }
  const anchoredAt = (node) => Array.from(byNode.get(node) ?? []);
  // Boundary points whose containers keep the index current however they are
  // assigned.
  function trackedPoints(document) {
    const state = { start: document, end: document };
    const points = {
      [POINTS]: POINTS,
      startOffset: 0,
      endOffset: 0,
      get startContainer() { return state.start; },
      set startContainer(node) { const old = state.start; state.start = node; release(old, points); anchor(node, points); },
      get endContainer() { return state.end; },
      set endContainer(node) { const old = state.end; state.end = node; release(old, points); anchor(node, points); },
    };
    anchor(document, points);
    return points;
  }
  function childIndex(node) {
    let index = 0;
    for (let sibling = node.previousSibling; sibling; sibling = sibling.previousSibling) index += 1;
    return index;
  }

  function root(node) {
    while (node.parentNode) node = node.parentNode;
    return node;
  }

  // A node's "length": the characters of character data, nothing for a
  // doctype, the children of anything else.
  function nodeLength(node) {
    if (DocumentType && node instanceof DocumentType) return 0;
    if (CharacterData && node instanceof CharacterData) return node.data.length;
    return node.childNodes.length;
  }
  // Web IDL `unsigned long`: -1 is 4294967295, past any end.
  function toOffset(value) {
    const number = Number(value);
    return Number.isFinite(number) ? Math.trunc(number) >>> 0 : 0;
  }
  // The checks every boundary point gets: a node, not a doctype, an offset
  // within its length.
  function checkPoint(node, offset) {
    if (!(node instanceof Node)) throw new TypeError("A range boundary must be in a Node");
    if (DocumentType && node instanceof DocumentType) rangeError("InvalidNodeTypeError", "A range boundary cannot be in a doctype.");
    offset = toOffset(offset);
    if (offset > nodeLength(node)) rangeError("IndexSizeError", "Range offset is past the end of its container.");
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

  function parentOf(node) {
    if (!(node instanceof Node)) throw new TypeError("A range boundary must be beside a Node");
    if (!node.parentNode) rangeError("InvalidNodeTypeError", "A root node has no position beside it.");
    return node.parentNode;
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

  const RANGE_DOCUMENT = Symbol("esdev DOM range document");
  class Range extends AbstractRange {
    static START_TO_START = 0;
    static START_TO_END = 1;
    static END_TO_END = 2;
    static END_TO_START = 3;
    // `new Range()` starts collapsed at the start of the current document, as
    // `document.createRange()` does for its own.
    constructor(document = globalThis.document) {
      const points = trackedPoints(document);
      super(points);
      Object.defineProperty(this, RANGE_DOCUMENT, { value: document });
      tracked += 1;
      collected.register(this, points);
    }
    get commonAncestorContainer() {
      const ancestors = new Set();
      for (let node = this.startContainer; node; node = node.parentNode) ancestors.add(node);
      for (let node = this.endContainer; node; node = node.parentNode) if (ancestors.has(node)) return node;
      return null;
    }
    // "Set the start or end": a point in another tree, or on the wrong side
    // of the other boundary, collapses the range to it rather than throwing.
    setStart(node, offset) {
      offset = checkPoint(node, offset);
      const points = this[POINTS];
      if (root(node) !== root(this.startContainer) || comparePoints(node, offset, this.endContainer, this.endOffset) > 0) {
        points.endContainer = node; points.endOffset = offset;
      }
      points.startContainer = node; points.startOffset = offset;
    }
    setEnd(node, offset) {
      offset = checkPoint(node, offset);
      const points = this[POINTS];
      if (root(node) !== root(this.startContainer) || comparePoints(node, offset, this.startContainer, this.startOffset) < 0) {
        points.startContainer = node; points.startOffset = offset;
      }
      points.endContainer = node; points.endOffset = offset;
    }
    setStartBefore(node) { this.setStart(parentOf(node), childIndex(node)); }
    setStartAfter(node) { this.setStart(parentOf(node), childIndex(node) + 1); }
    setEndBefore(node) { this.setEnd(parentOf(node), childIndex(node)); }
    setEndAfter(node) { this.setEnd(parentOf(node), childIndex(node) + 1); }
    selectNode(node) {
      const parent = parentOf(node);
      const index = childIndex(node);
      const points = this[POINTS];
      points.startContainer = parent; points.startOffset = index;
      points.endContainer = parent; points.endOffset = index + 1;
    }
    selectNodeContents(node) {
      if (!(node instanceof Node)) throw new TypeError("selectNodeContents expects a Node");
      if (DocumentType && node instanceof DocumentType) rangeError("InvalidNodeTypeError", "A range cannot select a doctype's contents.");
      const points = this[POINTS];
      points.startContainer = node; points.startOffset = 0;
      points.endContainer = node; points.endOffset = nodeLength(node);
    }
    collapse(toStart = false) {
      const points = this[POINTS];
      if (toStart) { points.endContainer = points.startContainer; points.endOffset = points.startOffset; }
      else { points.startContainer = points.endContainer; points.startOffset = points.endOffset; }
    }
    cloneRange() {
      const range = new Range(this[RANGE_DOCUMENT]);
      Object.assign(range[POINTS], {
        startContainer: this.startContainer, startOffset: this.startOffset,
        endContainer: this.endContainer, endOffset: this.endOffset,
      });
      return range;
    }
    compareBoundaryPoints(how, sourceRange) {
      // `unsigned short`, so 65536 is START_TO_START and -1 is not a mode.
      const number = Number(how);
      how = Number.isFinite(number) ? Math.trunc(number) & 0xFFFF : 0;
      if (how > 3) rangeError("NotSupportedError", "Unknown range comparison mode.");
      if (!(sourceRange instanceof Range)) throw new TypeError("compareBoundaryPoints expects a Range");
      if (root(this.startContainer) !== root(sourceRange.startContainer)) rangeError("WrongDocumentError", "The ranges are in different trees.");
      const [ours, theirs] = [
        [["start", "start"]], [["end", "start"]], [["end", "end"]], [["start", "end"]],
      ][how][0];
      return comparePoints(this[`${ours}Container`], this[`${ours}Offset`], sourceRange[`${theirs}Container`], sourceRange[`${theirs}Offset`]);
    }
    comparePoint(node, offset) {
      if (!(node instanceof Node)) throw new TypeError("comparePoint expects a Node");
      if (root(node) !== root(this.startContainer)) rangeError("WrongDocumentError", "The point is in a different tree.");
      offset = checkPoint(node, offset);
      if (comparePoints(node, offset, this.startContainer, this.startOffset) < 0) return -1;
      if (comparePoints(node, offset, this.endContainer, this.endOffset) > 0) return 1;
      return 0;
    }
    isPointInRange(node, offset) {
      if (!(node instanceof Node)) throw new TypeError("isPointInRange expects a Node");
      if (root(node) !== root(this.startContainer)) return false;
      offset = checkPoint(node, offset);
      return comparePoints(node, offset, this.startContainer, this.startOffset) >= 0
        && comparePoints(node, offset, this.endContainer, this.endOffset) <= 0;
    }
    intersectsNode(node) {
      if (!(node instanceof Node)) throw new TypeError("intersectsNode expects a Node");
      if (root(node) !== root(this.startContainer)) return false;
      const parent = node.parentNode;
      if (parent === null) return true;
      const offset = childIndex(node);
      return comparePoints(parent, offset, this.endContainer, this.endOffset) < 0
        && comparePoints(parent, offset + 1, this.startContainer, this.startOffset) > 0;
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
      // The specification's steps: a text start is split — even at its end,
      // which leaves an empty Text node, as Chrome does — and a collapsed
      // range grows to cover what was inserted.
      const container = this.startContainer;
      const collapsed = this.collapsed;
      let parent = container;
      let reference = container.childNodes.item(this.startOffset);
      if (container instanceof Text) {
        parent = container.parentNode;
        if (!parent) rangeError("HierarchyRequestError", "Cannot insert beside a detached text node.");
        reference = container.splitText(this.startOffset);
      }
      if (node === reference) reference = reference.nextSibling;
      if (node.parentNode) node.remove();
      let offset = reference ? childIndex(reference) : parent.childNodes.length;
      offset += node.nodeType === Node.DOCUMENT_FRAGMENT_NODE ? node.childNodes.length : 1;
      parent._preInsert(node, reference);
      if (collapsed) this.setEnd(parent, offset);
    }
    deleteContents() {
      if (this.collapsed) return;
      if (this.startContainer === this.endContainer && this.startContainer instanceof Text) {
        this.startContainer.deleteData(this.startOffset, this.endOffset - this.startOffset);
        return;
      }
      const start = { node: this.startContainer, offset: this.startOffset };
      const end = { node: this.endContainer, offset: this.endOffset };
      if (start.node instanceof Text) start.node.deleteData(start.offset, start.node.length - start.offset);
      if (end.node instanceof Text) end.node.deleteData(0, end.offset);
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
      for (const node of selected) node.parentNode._remove(node);
      this.setEnd(this.startContainer, this.startOffset);
    }
    cloneContents() {
      const fragment = this[RANGE_DOCUMENT].createDocumentFragment();
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
          return this[RANGE_DOCUMENT].createTextNode(node.data.slice(start, end));
        }
        const copy = node.cloneNode(false);
        for (let child = node.firstChild; child; child = child.nextSibling) {
          const selected = clone(child);
          if (selected) copy._insert(selected, null);
        }
        return copy.hasChildNodes() ? copy : null;
      };
      if (this.startContainer === this.endContainer && this.startContainer instanceof Text) {
        fragment._insert(this[RANGE_DOCUMENT].createTextNode(this.startContainer.data.slice(this.startOffset, this.endOffset)), null);
        return fragment;
      }
      const common = this.commonAncestorContainer;
      for (let child = common.firstChild; child; child = child.nextSibling) {
        const selected = clone(child);
        if (selected) fragment._insert(selected, null);
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
      node._preInsert(fragment, null);
      this.selectNode(node);
    }
    createContextualFragment(source) {
      const container = this.startContainer instanceof Element ? this.startContainer : this.startContainer.parentElement ?? this[RANGE_DOCUMENT].body;
      return parse.parseFragment(source, container);
    }
    // Long obsolete and specified to do nothing: a detached range still
    // follows the tree.
    detach() {}
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
      get() { return tracked > 0 ? adjust : null; },
      configurable: true,
    });
  }

  // The live-range steps of each tree and text mutation, over the ranges
  // anchored where it happened.
  const adjust = {
    insert(parent, index) {
      for (const points of anchoredAt(parent)) {
        if (points.startContainer === parent && points.startOffset > index) points.startOffset += 1;
        if (points.endContainer === parent && points.endOffset > index) points.endOffset += 1;
      }
    },
    // A boundary inside the removed subtree moves to where it was; one after
    // it in the parent moves back by one.
    remove(parent, node, index) {
      descendants(node, [node]).forEach((inside) => {
        for (const points of anchoredAt(inside)) {
          if (points.startContainer === inside) { points.startContainer = parent; points.startOffset = index; }
          if (points.endContainer === inside) { points.endContainer = parent; points.endOffset = index; }
        }
      });
      for (const points of anchoredAt(parent)) {
        if (points.startContainer === parent && points.startOffset > index) points.startOffset -= 1;
        if (points.endContainer === parent && points.endOffset > index) points.endOffset -= 1;
      }
    },
    // "Replace data": a boundary inside the replaced span moves to its start,
    // and one after it shifts by the change in length.
    replaceData(node, offset, count, added) {
      const end = offset + count;
      const shift = (at) => (at > end ? at + added - count : at > offset ? offset : at);
      for (const points of anchoredAt(node)) {
        if (points.startContainer === node) points.startOffset = shift(points.startOffset);
        if (points.endContainer === node) points.endOffset = shift(points.endOffset);
      }
    },
    // "Split a Text node": boundaries past the split move into the new node…
    splitText(node, tail, offset) {
      for (const points of anchoredAt(node)) {
        if (points.startContainer === node && points.startOffset > offset) { points.startOffset -= offset; points.startContainer = tail; }
        if (points.endContainer === node && points.endOffset > offset) { points.endOffset -= offset; points.endContainer = tail; }
      }
    },
    // …and, once the new node is inserted, a boundary in the parent just after
    // the old node moves past the new one too.
    afterSplit(parent, index) {
      for (const points of anchoredAt(parent)) {
        if (points.startContainer === parent && points.startOffset === index + 1) points.startOffset += 1;
        if (points.endContainer === parent && points.endOffset === index + 1) points.endOffset += 1;
      }
    },
  };

  defineConstants(Range, ["START_TO_START", "START_TO_END", "END_TO_END", "END_TO_START"]);

  return { AbstractRange, Range, StaticRange, install };
}
