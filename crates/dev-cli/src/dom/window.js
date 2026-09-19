// The sole public entry point for esdev's test-only DOM. Importing it installs
// a new realm-local document; the test runner imports it before the test entry.
import { createEvents } from "runtime:dom/events";
import { createTree } from "runtime:dom/tree";
import { createParsing } from "runtime:dom/parse";
import { createSelectors } from "runtime:dom/select";
import { createCss } from "runtime:dom/css";
import { createElements } from "runtime:dom/elements";

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

const windowEvents = new events.EventTarget();
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
    windowEvents.dispatchEvent(event);
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

Object.assign(globalThis, events, tree, css, elements, { document, customElements });
globalThis.window = globalThis;
Object.assign(globalThis, {
  History,
  IntersectionObserver: NeverObserver,
  Location,
  MediaQueryList,
  ResizeObserver: NeverObserver,
  Storage,
  cancelAnimationFrame,
  getComputedStyle,
  history,
  localStorage,
  location,
  matchMedia: (query) => new MediaQueryList(query),
  navigator,
  requestAnimationFrame,
  sessionStorage,
});
for (const method of ["addEventListener", "removeEventListener", "dispatchEvent"]) {
  globalThis[method] = windowEvents[method].bind(windowEvents);
}
