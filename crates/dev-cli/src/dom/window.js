// The sole public entry point for esdev's test-only DOM. Importing it installs
// a new realm-local document; the test runner imports it before the test entry.
import { createEvents } from "runtime:dom/events";
import { createTree } from "runtime:dom/tree";
import { createParsing } from "runtime:dom/parse";
import { createSelectors } from "runtime:dom/select";
import { createCss } from "runtime:dom/css";
import { createElements } from "runtime:dom/elements";
import { createRanges } from "runtime:dom/range";
import { createSheets } from "runtime:dom/sheets";

const events = createEvents();
const tree = createTree(events);
const parse = createParsing(
  tree,
  (source, context) => globalThis.__ops.dom_parse_fragment(source, context),
  (source) => globalThis.__ops.dom_parse_document(source),
);
const selectors = createSelectors(tree);
const css = createCss(tree);
const elements = createElements(tree);

// The document `new Text()` and friends belong to, before anything can use one.
const document = tree.setCurrentDocument(new tree.HTMLDocument());
// `<!doctype html>`, as a node: the starting document is the one the spec for
// this DOM names, and `document.doctype` is how code asks whether it is in
// standards mode.
document.appendChild(document.implementation.createDocumentType("html"));
const html = document.createElement("html");
const head = document.createElement("head");
const body = document.createElement("body");
html.append(head, body);
document.appendChild(html);

Object.defineProperties(document, {
  head: { get: () => head },
  body: { get: () => body },
  // Only this document has a window. One built by `DOMParser` or
  // `createHTMLDocument` keeps the prototype's `null`, as in a browser.
  defaultView: { get: () => globalThis },
});
parse.install();
selectors.install();
css.install();
const customElements = elements.install(document);
const ranges = createRanges(tree, parse);
ranges.install();
const sheets = createSheets({
  tree,
  parse: (text) => globalThis.__ops.dom_parse_stylesheet(text),
  selectors,
  css,
  mediaMatches,
});
sheets.install();

const NativeFormData = globalThis.FormData;
class DomFormData extends NativeFormData {
  constructor(form) {
    super();
    if (form === undefined) return;
    if (!(form instanceof tree.HTMLFormElement)) throw new TypeError("FormData constructor expects an HTMLFormElement");
    for (const control of form.elements) {
      // A form-associated custom element contributes whatever `setFormValue`
      // last gave it, under its own name.
      const custom = tree.formSubmissionValue(control);
      if (custom) { this.append(custom.name, custom.value); continue; }
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

// The focused element as *this* tree sees it. Focus inside a shadow root is
// reported to the document as the host; the root reports the element itself.
function focusedIn(scope) {
  let node = activeElement;
  while (node) {
    const root = node.getRootNode();
    if (root === scope) return node;
    if (!(root instanceof tree.ShadowRoot)) return scope === document ? node : null;
    node = root.host;
  }
  return null;
}

Object.defineProperty(document, "activeElement", { get: () => focusedIn(document) });
Object.defineProperty(tree.ShadowRoot.prototype, "activeElement", {
  get() { return focusedIn(this); },
  configurable: true,
});

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

// Writable and configurable, as every Web IDL operation is: a test library
// replaces `element.focus` to record calls, and an accessor that refuses the
// assignment breaks it.
Object.defineProperties(tree.HTMLElement.prototype, {
  focus: { value() { if (isFocusable(this)) changeFocus(this); }, writable: true, configurable: true },
  blur: { value() { if (activeElement === this) changeFocus(body); }, writable: true, configurable: true },
});
// The document's cookies, as a document-level string store. No network is
// involved and none is needed: a cookie is a name and a value with a lifetime,
// and code that sets one and reads it back — or, like a sanitiser, tests whether
// `"cookie" in document` to catch DOM clobbering — is asking this and nothing
// more. `Secure`, `Domain` and `Path` are accepted and ignored, because there is
// no origin to scope them to.
const cookies = new Map();

function writeCookie(text) {
  const [pair, ...attributes] = String(text).split(";");
  const equals = pair.indexOf("=");
  const name = (equals === -1 ? "" : pair.slice(0, equals)).trim();
  const value = (equals === -1 ? pair : pair.slice(equals + 1)).trim();
  if (name === "" && value === "") return;
  const expired = attributes.some((attribute) => {
    const [key, setting = ""] = attribute.split("=");
    const named = key.trim().toLowerCase();
    if (named === "max-age") return Number(setting.trim()) <= 0;
    if (named === "expires") {
      const when = Date.parse(setting.trim());
      return Number.isFinite(when) && when <= Date.now();
    }
    return false;
  });
  if (expired) cookies.delete(name);
  else cookies.set(name, value);
}

Object.defineProperties(document, {
  cookie: {
    get() { return Array.from(cookies, ([name, value]) => `${name}=${value}`).join("; "); },
    set(value) { writeCookie(value); },
    enumerable: true,
    configurable: true,
  },
  // A headless document is the focused one: there is no other, and a suite that
  // asks before dispatching key events should get on with it.
  hasFocus: { value: () => true, writable: true, configurable: true },
  visibilityState: { get: () => "visible", configurable: true },
  hidden: { get: () => false, configurable: true },
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
  // The parts are assignable, as in a browser: each one rewrites the URL in
  // place. Nothing navigates, so what changes is what `location` reports — and
  // for `hash`, which element `:target` matches.
  set protocol(value) { this.#part("protocol", value); }
  set host(value) { this.#part("host", value); }
  set hostname(value) { this.#part("hostname", value); }
  set port(value) { this.#part("port", value); }
  set pathname(value) { this.#part("pathname", value); }
  set search(value) { this.#part("search", value); }
  set hash(value) { this.#part("hash", value); }
  #part(name, value) {
    const next = new URL(this.#url.href);
    next[name] = String(value);
    this.#set(next.href);
  }
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

// A test DOM has no window to measure, so it declares one. The defaults are
// jsdom's, so a suite moved from there sees the same answers, and a test that
// cares assigns its own — nothing resizes on its own, so a media query is a
// pure function of this state.
const viewport = { width: 1024, height: 768, colorScheme: "light", reducedMotion: "no-preference", contrast: "no-preference", forcedColors: "none", pointer: "fine", hover: "hover", displayMode: "browser" };

function lengthInPixels(text) {
  const match = /^(-?\d*\.?\d+)(px|em|rem|pt|cm|mm|in|pc|q)?$/i.exec(String(text).trim());
  if (!match) return null;
  const value = Number(match[1]);
  const unit = (match[2] ?? "px").toLowerCase();
  const scale = { px: 1, em: 16, rem: 16, pt: 4 / 3, pc: 16, in: 96, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 101.6 }[unit] ?? 1;
  return value * scale;
}

// One `(feature: value)` or `(feature)`, answered from the declared viewport.
function featureMatches(text) {
  const condition = text.trim().replace(/^\(/, "").replace(/\)$/, "").trim();
  if (condition === "") return false;
  const colon = condition.indexOf(":");
  const name = (colon === -1 ? condition : condition.slice(0, colon)).trim().toLowerCase();
  const value = colon === -1 ? null : condition.slice(colon + 1).trim().toLowerCase();
  const bare = value === null;
  switch (name) {
    case "width": case "min-width": case "max-width": case "height": case "min-height": case "max-height": {
      const actual = name.endsWith("width") ? viewport.width : viewport.height;
      if (bare) return actual > 0;
      const wanted = lengthInPixels(value);
      if (wanted === null) return false;
      if (name.startsWith("min-")) return actual >= wanted;
      if (name.startsWith("max-")) return actual <= wanted;
      return actual === wanted;
    }
    case "orientation":
      return value === (viewport.width >= viewport.height ? "landscape" : "portrait");
    case "prefers-color-scheme":
      return bare ? true : value === viewport.colorScheme;
    case "prefers-reduced-motion":
      return bare ? viewport.reducedMotion !== "no-preference" : value === viewport.reducedMotion;
    case "prefers-contrast":
      return bare ? viewport.contrast !== "no-preference" : value === viewport.contrast;
    case "forced-colors":
      return bare ? viewport.forcedColors !== "none" : value === viewport.forcedColors;
    case "hover": case "any-hover":
      return bare ? viewport.hover !== "none" : value === viewport.hover;
    case "pointer": case "any-pointer":
      return bare ? viewport.pointer !== "none" : value === viewport.pointer;
    case "display-mode":
      return value === viewport.displayMode;
    case "scripting":
      return bare ? true : value === "enabled";
    default:
      // An unknown feature matches nothing, which is what a browser does with
      // one it does not implement.
      return false;
  }
}

function queryMatches(query) {
  const text = String(query).trim().toLowerCase();
  if (text === "" || text === "all") return true;
  if (text.startsWith("not ")) return !queryMatches(text.slice(4));
  if (text.startsWith("only ")) return queryMatches(text.slice(5));
  const parts = [];
  let depth = 0;
  let start = 0;
  for (let at = 0; at <= text.length; at += 1) {
    const char = text[at];
    if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if ((at === text.length || /\s/.test(char)) && depth === 0) {
      const word = text.slice(start, at).trim();
      if (word) parts.push(word);
      start = at + 1;
    }
  }
  let matched = true;
  for (const part of parts) {
    if (part === "and") continue;
    if (part === "screen" || part === "all") continue;
    // Any other media type — print, speech — is not this one.
    if (!part.startsWith("(")) {
      if (/^[a-z-]+$/.test(part)) return false;
      return false;
    }
    matched = matched && featureMatches(part);
  }
  return matched;
}

// A comma-separated list matches when any of its queries does.
function mediaMatches(list) {
  const text = String(list).trim();
  if (text === "") return true;
  return text.split(",").some((query) => queryMatches(query));
}

class MediaQueryList extends events.EventTarget {
  constructor(media) {
    super();
    this.media = String(media);
    Object.defineProperty(this, "matches", { get: () => mediaMatches(this.media), enumerable: true });
  }
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

// Two interfaces, not one class under two names: `new ResizeObserver()` is not
// an `IntersectionObserver`, and each stringifies as itself.
class IntersectionObserver extends NeverObserver {}
class ResizeObserver extends NeverObserver {}

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

// Web IDL puts the interface's own name on its prototype as
// `Symbol.toStringTag`, which is what `Object.prototype.toString.call(node)`
// reports. Without it every node is `[object Object]`, and a logger or a type
// guard that leans on the tag cannot tell a comment from a plain object.
// Capitalised keys only: a factory also exports helper functions, and those are
// not interfaces.
function nameInterfaces(source) {
  for (const [name, value] of Object.entries(source)) {
    if (typeof value !== "function" || !/^[A-Z]/.test(name)) continue;
    const { prototype } = value;
    if (!prototype || Object.hasOwn(prototype, Symbol.toStringTag)) continue;
    Object.defineProperty(prototype, Symbol.toStringTag, { value: name, configurable: true });
  }
}

Object.assign(globalThis, events, tree, css, elements, { document, customElements });
globalThis.window = globalThis;
const globals = {
  DOMParser: parse.DOMParser,
  History,
  FormData: DomFormData,
  IntersectionObserver,
  Location,
  MediaQueryList,
  MutationObserver,
  ResizeObserver,
  AbstractRange: ranges.AbstractRange,
  CSS: Object.freeze({
    supports(property, value) {
      // The one-argument form takes a whole condition, as in `@supports`.
      if (value === undefined) return sheets.supportsCondition(String(property));
      return css.supportsDeclaration(String(property), String(value));
    },
    escape(text) { return String(text).replace(/[^\w-]/g, (character) => `\\${character}`); },
  }),
  CSSRule: sheets.CSSRule,
  CSSRuleList: sheets.CSSRuleList,
  StyleSheetList: sheets.StyleSheetList,
  CSSStyleRule: sheets.CSSStyleRule,
  CSSStyleSheet: sheets.CSSStyleSheet,
  CSSGroupingRule: sheets.CSSGroupingRule,
  Range: ranges.Range,
  StaticRange: ranges.StaticRange,
  Selection,
  Storage,
  cancelAnimationFrame,
  console: browserConsole,
  getComputedStyle: sheets.getComputedStyle,
  history,
  localStorage,
  location,
  matchMedia: (query) => new MediaQueryList(query),
  navigator,
  requestAnimationFrame,
  sessionStorage,
  getSelection: () => selection,
};
Object.assign(globalThis, globals);

nameInterfaces(events);
nameInterfaces(tree);
nameInterfaces(css);
nameInterfaces(elements);
nameInterfaces(globals);
Object.defineProperty(globalThis, Symbol.toStringTag, { value: "Window", configurable: true });

// Accessors rather than values in the assignment above: `Object.assign` would
// have called the getter and left a plain number behind, so a test assigning to
// `innerWidth` would change nothing a media query reads.
Object.defineProperties(globalThis, {
  innerWidth: {
    get: () => viewport.width,
    set(value) { viewport.width = Number(value); },
    enumerable: true,
    configurable: true,
  },
  innerHeight: {
    get: () => viewport.height,
    set(value) { viewport.height = Number(value); },
    enumerable: true,
    configurable: true,
  },
  // The window's own geometry and scrolling, zero and inert for the same reason
  // an element's is: there is no rendered page to scroll.
  scrollX: { get: () => 0, enumerable: true, configurable: true },
  scrollY: { get: () => 0, enumerable: true, configurable: true },
  pageXOffset: { get: () => 0, enumerable: true, configurable: true },
  pageYOffset: { get: () => 0, enumerable: true, configurable: true },
  scroll: { value() {}, writable: true, configurable: true },
  scrollTo: { value() {}, writable: true, configurable: true },
  scrollBy: { value() {}, writable: true, configurable: true },
  outerWidth: { get: () => viewport.width, enumerable: true, configurable: true },
  outerHeight: { get: () => viewport.height, enumerable: true, configurable: true },
  devicePixelRatio: { get: () => 1, enumerable: true, configurable: true },
});
