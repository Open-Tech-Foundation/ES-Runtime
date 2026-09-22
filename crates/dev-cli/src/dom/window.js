// The sole public entry point for esdev's test-only DOM. Importing it installs
// a new realm-local document; the test runner imports it before the test entry.
import { createEvents } from "runtime:dom/events";
import { createTree } from "runtime:dom/tree";
import { createParsing } from "runtime:dom/parse";
import { createSelectors } from "runtime:dom/select";
import { createCss } from "runtime:dom/css";
import { createElements } from "runtime:dom/elements";
import { createRanges } from "runtime:dom/range";

const events = createEvents();
const tree = createTree(events);
const parse = createParsing(tree, (source, context) =>
  globalThis.__ops.dom_parse_fragment(source, context));
const selectors = createSelectors(tree);
const css = createCss(tree);
const elements = createElements(tree);

const document = new tree.Document();
const html = document.createElement("html");
const head = document.createElement("head");
const body = document.createElement("body");
html.append(head, body);
document.appendChild(html);

Object.defineProperties(document, {
  head: { get: () => head },
  body: { get: () => body },
});
parse.install();
selectors.install();
css.install();
const customElements = elements.install(document);
const ranges = createRanges(tree, parse);
ranges.install(document);

const NativeFormData = globalThis.FormData;
class DomFormData extends NativeFormData {
  constructor(form) {
    super();
    if (form === undefined) return;
    if (!(form instanceof tree.HTMLFormElement)) throw new TypeError("FormData constructor expects an HTMLFormElement");
    for (const control of form.elements) {
      if (tree.isDisabled(control) || !control.name || control instanceof tree.HTMLButtonElement || control instanceof tree.HTMLFieldSetElement) continue;
      if (control instanceof tree.HTMLInputElement && ["checkbox", "radio"].includes(control.type) && !control.checked) continue;
      if (control instanceof tree.HTMLSelectElement) {
        for (const option of control.selectedOptions) if (!tree.isDisabled(option)) this.append(control.name, option.value);
      } else {
        this.append(control.name, control.value);
      }
    }
  }
}

let activeElement = body;
Object.defineProperty(document, "activeElement", { get: () => activeElement });

function isFocusable(element) {
  if (!element.isConnected || element.disabled || element.localName === "input" && element.type === "hidden") return false;
  if (element.hasAttribute("tabindex")) return true;
  if (["button", "input", "select", "textarea", "iframe"].includes(element.localName)) return true;
  return ["a", "area"].includes(element.localName) && element.hasAttribute("href");
}

function changeFocus(next) {
  const previous = activeElement;
  if (previous === next) return;
  activeElement = next;
  if (previous) {
    previous.dispatchEvent(new events.FocusEvent("blur", { relatedTarget: next }));
    previous.dispatchEvent(new events.FocusEvent("focusout", { bubbles: true, relatedTarget: next }));
  }
  if (next) {
    next.dispatchEvent(new events.FocusEvent("focus", { relatedTarget: previous }));
    next.dispatchEvent(new events.FocusEvent("focusin", { bubbles: true, relatedTarget: previous }));
  }
}

Object.defineProperties(tree.HTMLElement.prototype, {
  focus: { value() { if (isFocusable(this)) changeFocus(this); } },
  blur: { value() { if (activeElement === this) changeFocus(body); } },
});
Object.defineProperty(document, "_activeElementRemoved", {
  value(node) {
    for (let current = activeElement; current; current = current.parentNode) {
      if (current === node) { changeFocus(body); return; }
    }
  },
});

class Storage {
  #values = new Map();
  get length() { return this.#values.size; }
  key(index) { return Array.from(this.#values.keys())[Number(index)] ?? null; }
  getItem(key) { return this.#values.get(String(key)) ?? null; }
  setItem(key, value) { this.#values.set(String(key), String(value)); }
  removeItem(key) { this.#values.delete(String(key)); }
  clear() { this.#values.clear(); }
}

class Location {
  #url = new URL("http://localhost/");
  #set(value) { this.#url = new URL(String(value), this.#url.href); }
  get href() { return this.#url.href; }
  set href(value) { this.#set(value); }
  get origin() { return this.#url.origin; }
  get protocol() { return this.#url.protocol; }
  get host() { return this.#url.host; }
  get hostname() { return this.#url.hostname; }
  get port() { return this.#url.port; }
  get pathname() { return this.#url.pathname; }
  get search() { return this.#url.search; }
  get hash() { return this.#url.hash; }
  assign(value) { this.#set(value); }
  replace(value) { this.#set(value); }
  reload() {}
}

// `window` is the global object and the last entry in every propagation path.
events.asEventTarget(globalThis);
// A Document's event parent is its window, so an event dispatched in the tree
// reaches a `window.addEventListener` listener and appears in `composedPath()`.
// `load` is the documented exception: the window's own load event is not the
// document's, so the document's does not propagate to it.
Object.defineProperty(document, "_eventParent", {
  value: (event) => (event.type === "load" ? null : globalThis),
});
const location = new Location();
class History {
  #entries = [{ state: null, href: location.href }];
  #at = 0;
  get length() { return this.#entries.length; }
  get state() { return this.#entries[this.#at].state; }
  #url(value) { return value === undefined || value === null ? location.href : new URL(String(value), location.href).href; }
  #move(index) {
    if (index < 0 || index >= this.#entries.length || index === this.#at) return;
    this.#at = index;
    location.assign(this.#entries[index].href);
    const event = new events.Event("popstate");
    event.state = this.state;
    globalThis.dispatchEvent(event);
  }
  pushState(state, _unused, url) {
    this.#entries.splice(this.#at + 1);
    this.#entries.push({ state, href: this.#url(url) });
    this.#at = this.#entries.length - 1;
    location.assign(this.#entries[this.#at].href);
  }
  replaceState(state, _unused, url) {
    this.#entries[this.#at] = { state, href: this.#url(url) };
    location.assign(this.#entries[this.#at].href);
  }
  back() { this.#move(this.#at - 1); }
  forward() { this.#move(this.#at + 1); }
  go(delta = 0) { this.#move(this.#at + Number(delta)); }
}

class Selection {
  #ranges = [];
  get rangeCount() { return this.#ranges.length; }
  get anchorNode() { return this.#ranges[0]?.startContainer ?? null; }
  get anchorOffset() { return this.#ranges[0]?.startOffset ?? 0; }
  get focusNode() { return this.#ranges[0]?.endContainer ?? null; }
  get focusOffset() { return this.#ranges[0]?.endOffset ?? 0; }
  get isCollapsed() { return this.#ranges.length === 0 || this.#ranges.every((range) => range.collapsed); }
  addRange(range) {
    if (!(range instanceof ranges.Range)) throw new TypeError("Selection.addRange expects a Range");
    if (range.document !== document) throw new DOMException("The range belongs to another document.", "WrongDocumentError");
    this.#ranges = [range];
  }
  removeAllRanges() { this.#ranges = []; }
  removeRange(range) { this.#ranges = this.#ranges.filter((candidate) => candidate !== range); }
  getRangeAt(index) {
    const range = this.#ranges[Number(index)];
    if (!range) throw new DOMException("The range index is out of bounds.", "IndexSizeError");
    return range;
  }
  collapse(node, offset = 0) {
    if (node === null) return this.removeAllRanges();
    const range = new ranges.Range(document);
    range.setStart(node, offset); range.collapse(true);
    this.#ranges = [range];
  }
}

class MediaQueryList extends events.EventTarget {
  constructor(media) { super(); this.media = String(media); this.matches = false; }
  addListener(listener) { this.addEventListener("change", listener); }
  removeListener(listener) { this.removeEventListener("change", listener); }
}

class NeverObserver {
  constructor(callback) {
    if (typeof callback !== "function") throw new TypeError("Observer callback must be a function");
    this.callback = callback;
  }
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() { return []; }
}

class SnapshotNodeList {
  constructor(values) {
    this.values = [...values];
    for (const [index, value] of this.values.entries()) this[index] = value;
  }
  get length() { return this.values.length; }
  item(index) { return this.values[Number(index)] ?? null; }
  [Symbol.iterator]() { return this.values[Symbol.iterator](); }
}

const mutationObservers = new Set();
let mutationDeliveryQueued = false;
function scheduleMutationDelivery() {
  if (mutationDeliveryQueued) return;
  mutationDeliveryQueued = true;
  queueMicrotask(() => {
    mutationDeliveryQueued = false;
    for (const observer of mutationObservers) {
      const records = observer.takeRecords();
      if (records.length) observer.callback(records, observer);
    }
  });
}

class MutationObserver {
  constructor(callback) {
    if (typeof callback !== "function") throw new TypeError("MutationObserver callback must be a function");
    this.callback = callback;
    this.records = [];
    this.registrations = new Map();
    mutationObservers.add(this);
  }
  observe(target, options = {}) {
    if (!(target instanceof tree.Node)) throw new TypeError("MutationObserver target must be a Node");
    const settings = { ...options };
    if (settings.attributeOldValue || settings.attributeFilter) settings.attributes ??= true;
    if (settings.characterDataOldValue) settings.characterData ??= true;
    for (const type of ["attributes", "childList", "characterData", "subtree"]) settings[type] = Boolean(settings[type]);
    if (!settings.attributes && !settings.childList && !settings.characterData) throw new TypeError("MutationObserver must observe at least one mutation type");
    if (settings.attributeOldValue && !settings.attributes) throw new TypeError("attributeOldValue requires attributes");
    if (settings.attributeFilter && !settings.attributes) throw new TypeError("attributeFilter requires attributes");
    if (settings.characterDataOldValue && !settings.characterData) throw new TypeError("characterDataOldValue requires characterData");
    if (settings.attributeFilter) settings.attributeFilter = Array.from(settings.attributeFilter, String);
    this.registrations.set(target, settings);
  }
  disconnect() { this.registrations.clear(); this.records.length = 0; }
  takeRecords() { const records = this.records; this.records = []; return records; }
  _enqueue(change) {
    let settings;
    for (let current = change.target; current; current = current.parentNode) {
      const candidate = this.registrations.get(current);
      if (candidate && (current === change.target || candidate.subtree)) { settings = candidate; break; }
    }
    if (!settings || !settings[change.type]) return;
    if (change.type === "attributes" && settings.attributeFilter && !settings.attributeFilter.includes(change.attributeName)) return;
    this.records.push({
      type: change.type,
      target: change.target,
      addedNodes: new SnapshotNodeList(change.addedNodes ?? []),
      removedNodes: new SnapshotNodeList(change.removedNodes ?? []),
      previousSibling: change.previousSibling ?? null,
      nextSibling: change.nextSibling ?? null,
      attributeName: change.attributeName ?? null,
      attributeNamespace: null,
      oldValue: change.type === "attributes" ? settings.attributeOldValue ? change.oldValue : null : change.type === "characterData" && settings.characterDataOldValue ? change.oldValue : null,
    });
    scheduleMutationDelivery();
  }
}

Object.defineProperty(document, "_queueMutation", {
  value(change) { for (const observer of mutationObservers) observer._enqueue(change); },
});

function getComputedStyle(element) {
  const source = element.style;
  const read = {
    get length() { return source.length; },
    item(index) { return source.item(index); },
    getPropertyValue(name) { return source.getPropertyValue(name); },
    getPropertyPriority(name) { return source.getPropertyPriority(name); },
    get cssText() { return source.cssText; },
  };
  return new Proxy(read, {
    get(target, property, receiver) {
      if (typeof property === "string" && !(property in target)) return source[property];
      return Reflect.get(target, property, receiver);
    },
    set() { throw new TypeError("Computed styles are read-only"); },
  });
}

let nextAnimationFrame = 1;
const animationFrames = new Map();
function requestAnimationFrame(callback) {
  if (typeof callback !== "function") throw new TypeError("requestAnimationFrame callback must be a function");
  const id = nextAnimationFrame++;
  const timer = setTimeout(() => {
    animationFrames.delete(id);
    callback(Date.now());
  }, 16);
  animationFrames.set(id, timer);
  return id;
}
function cancelAnimationFrame(id) {
  const timer = animationFrames.get(Number(id));
  if (timer !== undefined) clearTimeout(timer);
  animationFrames.delete(Number(id));
}

const navigator = Object.freeze({
  userAgent: "esdev DOM",
  language: "en-US",
  languages: Object.freeze(["en-US"]),
});
const history = new History();
const localStorage = new Storage();
const sessionStorage = new Storage();
const selection = new Selection();
Object.defineProperty(document, "getSelection", { value: () => selection });
const hostConsole = globalThis.console;
const browserConsole = Object.create(null);
for (const method of Object.getOwnPropertyNames(hostConsole)) {
  const value = hostConsole[method];
  Object.defineProperty(browserConsole, method, {
    value: typeof value === "function" ? value.bind(hostConsole) : value,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

Object.assign(globalThis, events, tree, css, elements, { document, customElements });
globalThis.window = globalThis;
Object.assign(globalThis, {
  History,
  FormData: DomFormData,
  IntersectionObserver: NeverObserver,
  Location,
  MediaQueryList,
  MutationObserver,
  ResizeObserver: NeverObserver,
  Range: ranges.Range,
  Selection,
  Storage,
  cancelAnimationFrame,
  console: browserConsole,
  getComputedStyle,
  history,
  localStorage,
  location,
  matchMedia: (query) => new MediaQueryList(query),
  navigator,
  requestAnimationFrame,
  sessionStorage,
  getSelection: () => selection,
});
