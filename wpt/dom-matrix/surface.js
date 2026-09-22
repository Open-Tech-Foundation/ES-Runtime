// The DOM surface, probed feature by feature so the same questions can be put
// to Chrome, esdev, jsdom and happy-dom.
//
// Every entry answers with a boolean or a short string, and answers about an
// *instance* rather than a prototype: a member can be an own property, and an
// interface global can be missing while the behaviour behind it is there. Those
// are two different questions, so they are asked separately.

const has = (object, name) => {
  try {
    return object != null && name in object;
  } catch {
    return false;
  }
};

const global = (window, name) => has(window, name) && window[name] != null;

const make = (window, tag) => window.document.createElement(tag);

const sample = {
  Node: (w) => make(w, "div"),
  Element: (w) => make(w, "div"),
  HTMLElement: (w) => make(w, "div"),
  HTMLFormElement: (w) => make(w, "form"),
  HTMLInputElement: (w) => make(w, "input"),
  HTMLSelectElement: (w) => make(w, "select"),
  HTMLFieldSetElement: (w) => make(w, "fieldset"),
  HTMLTemplateElement: (w) => make(w, "template"),
  HTMLSlotElement: (w) => make(w, "slot"),
  HTMLDialogElement: (w) => make(w, "dialog"),
  HTMLTableElement: (w) => make(w, "table"),
  HTMLTableRowElement: (w) => make(w, "tr"),
  HTMLStyleElement: (w) => make(w, "style"),
  Range: (w) => w.document.createRange(),
};

const method = (window, ctor, name) => {
  try {
    return typeof sample[ctor](window)[name] === "function";
  } catch {
    return false;
  }
};

const accessor = (window, ctor, name) => {
  try {
    return name in sample[ctor](window);
  } catch {
    return false;
  }
};

function value(callback) {
  try {
    const result = callback();
    if (typeof result === "string") return result;
    if (typeof result === "number") return String(result);
    if (typeof result === "boolean") return result;
    return JSON.stringify(result) ?? "undefined";
  } catch (error) {
    return `throws:${error?.name ?? "Error"}`;
  }
}

// The name of the error a call throws, or null when it does not.
function errorName(callback) {
  try {
    callback();
    return null;
  } catch (error) {
    return error?.name ?? "Error";
  }
}

// A connected element, because a computed style is only answered for one.
function connected(window, tag = "div") {
  const element = make(window, tag);
  window.document.body.append(element);
  return element;
}

export const features = [
  // --- core tree -----------------------------------------------------------
  ["tree", "Node", (w) => global(w, "Node")],
  ["tree", "Element", (w) => global(w, "Element")],
  ["tree", "DocumentFragment", (w) => global(w, "DocumentFragment")],
  ["tree", "Attr", (w) => global(w, "Attr")],
  ["tree", "Comment", (w) => global(w, "Comment")],
  ["tree", "CDATASection", (w) => global(w, "CDATASection")],
  ["tree", "ProcessingInstruction", (w) => global(w, "ProcessingInstruction")],
  ["tree", "DocumentType", (w) => global(w, "DocumentType")],
  ["tree", "DOMImplementation", (w) => global(w, "DOMImplementation")],
  ["tree", "NamedNodeMap", (w) => global(w, "NamedNodeMap")],
  ["tree", "HTMLCollection", (w) => global(w, "HTMLCollection")],
  ["tree", "NodeList", (w) => global(w, "NodeList")],
  ["tree", "DOMTokenList", (w) => global(w, "DOMTokenList")],
  ["tree", "DOMStringMap", (w) => global(w, "DOMStringMap")],
  ["tree", "CharacterData in the prototype chain", (w) => value(() => {
    let proto = Object.getPrototypeOf(w.document.createTextNode("x"));
    const chain = [];
    while (proto) { chain.push(proto.constructor?.name ?? "?"); proto = Object.getPrototypeOf(proto); }
    return chain.join(" < ");
  })],
  ["tree", "document.implementation.createHTMLDocument", (w) => value(() => !!w.document.implementation.createHTMLDocument("t").body)],
  ["tree", "document.doctype", (w) => value(() => w.document.doctype?.name ?? "none")],
  ["tree", "document.title", (w) => value(() => typeof w.document.title === "string")],
  ["tree", "document.defaultView", (w) => value(() => w.document.defaultView === w)],
  ["tree", "Node.compareDocumentPosition", (w) => method(w, "Node", "compareDocumentPosition")],
  ["tree", "Node.isEqualNode", (w) => method(w, "Node", "isEqualNode")],
  ["tree", "Node.normalize", (w) => method(w, "Node", "normalize")],
  ["tree", "Node.getRootNode", (w) => method(w, "Node", "getRootNode")],
  ["tree", "Node.contains", (w) => method(w, "Node", "contains")],
  ["tree", "Element.closest", (w) => method(w, "Element", "closest")],
  ["tree", "Element.matches", (w) => method(w, "Element", "matches")],
  ["tree", "Element.insertAdjacentHTML", (w) => method(w, "Element", "insertAdjacentHTML")],
  ["tree", "Element.insertAdjacentElement", (w) => method(w, "Element", "insertAdjacentElement")],
  ["tree", "Element.toggleAttribute", (w) => method(w, "Element", "toggleAttribute")],
  ["tree", "Element.getAttributeNames", (w) => method(w, "Element", "getAttributeNames")],
  ["tree", "Element.setHTMLUnsafe", (w) => method(w, "Element", "setHTMLUnsafe")],
  ["tree", "Element.getHTML", (w) => method(w, "Element", "getHTML")],
  ["tree", "Element.checkVisibility", (w) => method(w, "Element", "checkVisibility")],
  ["tree", "document.adoptNode", (w) => typeof w.document.adoptNode === "function"],
  ["tree", "document.importNode", (w) => typeof w.document.importNode === "function"],
  ["tree", "document.firstElementChild", (w) => value(() => w.document.firstElementChild === w.document.documentElement)],
  ["tree", "innerText", (w) => accessor(w, "HTMLElement", "innerText")],
  ["tree", "outerHTML", (w) => accessor(w, "Element", "outerHTML")],
  ["tree", "table.rows", (w) => accessor(w, "HTMLTableElement", "rows")],
  ["tree", "table.tBodies", (w) => accessor(w, "HTMLTableElement", "tBodies")],
  ["tree", "row.cells", (w) => accessor(w, "HTMLTableRowElement", "cells")],
  ["tree", "row.rowIndex", (w) => accessor(w, "HTMLTableRowElement", "rowIndex")],
  ["tree", "HTMLTableSectionElement", (w) => global(w, "HTMLTableSectionElement")],
  ["tree", "createElement lowercases", (w) => value(() => `${make(w, "DIV").localName}/${make(w, "DIV").tagName}`)],
  ["tree", "uppercase markup", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<SPAN CLASS=a>x</SPAN>";
    return host.innerHTML;
  })],
  ["tree", "new DocumentFragment()", (w) => value(() => {
    const fragment = new w.DocumentFragment();
    fragment.append(new w.Text("x"));
    const host = connected(w, "div");
    host.append(fragment);
    const answer = host.innerHTML;
    host.remove();
    return answer;
  })],
  ["tree", "fragment.getElementById", (w) => value(() => {
    const fragment = new w.DocumentFragment();
    const inside = make(w, "b");
    inside.id = "probe-in-fragment";
    fragment.append(inside);
    return typeof fragment.getElementById === "function" ? fragment.getElementById("probe-in-fragment") === inside : "missing";
  })],
  ["tree", "document.hasFocus()", (w) => value(() => w.document.hasFocus())],
  ["tree", "element.focus is writable", (w) => value(() => {
    const element = make(w, "button");
    try {
      element.focus = () => {};
      return typeof element.focus === "function";
    } catch (error) {
      return error.name;
    }
  })],
  ["traversal", "NodeIterator walks", (w) => value(() => {
    if (typeof w.document.createNodeIterator !== "function") return "missing";
    const host = make(w, "div");
    host.innerHTML = "<a>1</a><b><i>2</i></b>";
    const iterator = w.document.createNodeIterator(host, w.NodeFilter.SHOW_ELEMENT);
    const seen = [];
    for (let node = iterator.nextNode(); node; node = iterator.nextNode()) seen.push(node.localName);
    return seen.join(",");
  })],
  ["tree", "implementation.createDocument", (w) => value(() => {
    const made = w.document.implementation.createDocument("http://www.w3.org/2000/svg", "svg", null);
    return `${made.documentElement.localName}/${made.documentElement.namespaceURI}`;
  })],
  ["tree", "a parsed document has ranges and sheets", (w) => value(() => {
    const parsed = new w.DOMParser().parseFromString("<p>x</p>", "text/html");
    return `${typeof parsed.createRange}/${typeof parsed.styleSheets?.length}`;
  })],
  ["selectors", "escaped identifier", (w) => value(() => {
    const host = connected(w, "div");
    host.innerHTML = '<a id="probe.dotted">1</a>';
    const found = errorName(() => host.querySelector("#probe\\.dotted"));
    const answer = found ?? (host.querySelector("#probe\\.dotted")?.textContent ?? "null");
    host.remove();
    return answer;
  })],
  ["cascade", "unknown property is undefined", (w) => value(() => {
    const element = make(w, "div");
    return `${element.style.nonsenseProp}/${"nonsenseProp" in element.style}/${w.CSS?.supports?.("nonsense-prop", "1")}`;
  })],
  ["cascade", "keyword initial values", (w) => value(() => {
    const element = connected(w, "div");
    const computed = w.getComputedStyle(element);
    const answer = [computed.transform, computed.opacity, computed.overflow, computed.marginTop, computed.zIndex].join(",");
    element.remove();
    return answer;
  })],
  ["window", "document.cookie is a store", (w) => value(() => "cookie" in w.document)],

  // --- traversal and ranges ------------------------------------------------
  ["traversal", "Range", (w) => global(w, "Range")],
  ["traversal", "StaticRange", (w) => global(w, "StaticRange")],
  ["traversal", "AbstractRange", (w) => global(w, "AbstractRange")],
  ["traversal", "document.createRange", (w) => typeof w.document.createRange === "function"],
  ["traversal", "Range.createContextualFragment", (w) => method(w, "Range", "createContextualFragment")],
  ["traversal", "Range.getBoundingClientRect", (w) => method(w, "Range", "getBoundingClientRect")],
  ["tree", "DOMRect", (w) => global(w, "DOMRect")],
  ["cascade", "element scroll offsets", (w) => value(() => {
    const element = connected(w, "div");
    element.scrollTop = 40;
    const answer = [element.scrollTop, element.scrollLeft, element.offsetParent === null].join(",");
    element.remove();
    return answer;
  })],
  ["traversal", "readonly range boundary points", (w) => value(() => {
    const range = w.document.createRange();
    try {
      range.startOffset = 9;
      return range.startOffset === 9 ? "writable" : "ignored";
    } catch (error) {
      return error.name;
    }
  })],
  ["traversal", "NodeIterator", (w) => global(w, "NodeIterator")],
  ["traversal", "TreeWalker", (w) => global(w, "TreeWalker")],
  ["traversal", "NodeFilter", (w) => global(w, "NodeFilter")],
  ["traversal", "XPathEvaluator (document.evaluate)", (w) => typeof w.document.evaluate === "function"],
  ["traversal", "Selection", (w) => global(w, "Selection")],
  ["traversal", "getSelection()", (w) => typeof w.getSelection === "function"],

  // --- events --------------------------------------------------------------
  ["events", "EventTarget", (w) => global(w, "EventTarget")],
  ["events", "Event", (w) => global(w, "Event")],
  ["events", "CustomEvent", (w) => global(w, "CustomEvent")],
  ["events", "UIEvent", (w) => global(w, "UIEvent")],
  ["events", "MouseEvent", (w) => global(w, "MouseEvent")],
  ["events", "PointerEvent", (w) => global(w, "PointerEvent")],
  ["events", "KeyboardEvent", (w) => global(w, "KeyboardEvent")],
  ["events", "InputEvent", (w) => global(w, "InputEvent")],
  ["events", "FocusEvent", (w) => global(w, "FocusEvent")],
  ["events", "SubmitEvent", (w) => global(w, "SubmitEvent")],
  ["events", "WheelEvent", (w) => global(w, "WheelEvent")],
  ["events", "DragEvent", (w) => global(w, "DragEvent")],
  ["events", "ClipboardEvent", (w) => global(w, "ClipboardEvent")],
  ["events", "TouchEvent", (w) => global(w, "TouchEvent")],
  ["events", "ErrorEvent", (w) => global(w, "ErrorEvent")],
  ["events", "MessageEvent", (w) => global(w, "MessageEvent")],
  ["events", "AbortController", (w) => global(w, "AbortController")],
  ["events", "createEvent by interface name", (w) => value(() => {
    if (typeof w.document.createEvent !== "function") return "missing";
    const event = w.document.createEvent("Event");
    const uninitialized = errorName(() => w.document.body.dispatchEvent(event)) ?? "dispatched";
    event.initEvent("probe-created", true, false);
    return `${event.constructor.name}/${uninitialized}/${event.type}/${event.bubbles}`;
  })],
  ["events", "createEvent refuses the HTML4 aliases", (w) => value(() => {
    if (typeof w.document.createEvent !== "function") return "missing";
    return errorName(() => w.document.createEvent("HTMLEvents")) ?? "created";
  })],
  ["events", "listener signal option", (w) => value(() => {
    const target = new w.EventTarget();
    const controller = new w.AbortController();
    let calls = 0;
    target.addEventListener("x", () => calls++, { signal: controller.signal });
    controller.abort();
    target.dispatchEvent(new w.Event("x"));
    return calls === 0;
  })],
  ["events", "window is in the propagation path", (w) => value(() => {
    const target = connected(w, "i");
    let seen = 0;
    const listener = () => seen++;
    w.addEventListener("esdev-probe", listener);
    target.dispatchEvent(new w.Event("esdev-probe", { bubbles: true }));
    w.removeEventListener("esdev-probe", listener);
    target.remove();
    return seen === 1;
  })],
  ["events", "composedPath() through a shadow root", (w) => value(() => {
    const host = connected(w, "div");
    const root = host.attachShadow({ mode: "open" });
    const inner = make(w, "span");
    root.append(inner);
    let length = 0;
    host.addEventListener("x", (event) => { length = event.composedPath().length; });
    inner.dispatchEvent(new w.Event("x", { bubbles: true, composed: true }));
    host.remove();
    return length;
  })],
  ["events", "MutationObserver", (w) => global(w, "MutationObserver")],
  ["events", "MutationObserver delivers records", (w) => value(() => {
    const target = make(w, "div");
    const observer = new w.MutationObserver(() => {});
    observer.observe(target, { childList: true });
    target.append(make(w, "i"));
    const records = observer.takeRecords();
    observer.disconnect();
    return records.length > 0;
  })],
  ["events", "IntersectionObserver", (w) => global(w, "IntersectionObserver")],
  ["events", "ResizeObserver", (w) => global(w, "ResizeObserver")],

  // --- shadow DOM and custom elements --------------------------------------
  ["components", "ShadowRoot", (w) => global(w, "ShadowRoot")],
  ["components", "attachShadow", (w) => method(w, "Element", "attachShadow")],
  ["components", "closed mode hides the root", (w) => value(() => {
    const host = make(w, "div");
    host.attachShadow({ mode: "closed" });
    return host.shadowRoot === null;
  })],
  ["components", "HTMLSlotElement", (w) => global(w, "HTMLSlotElement")],
  ["components", "slot.assignedNodes", (w) => method(w, "HTMLSlotElement", "assignedNodes")],
  ["components", "manual slot assignment", (w) => value(() => {
    const host = make(w, "div");
    const root = host.attachShadow({ mode: "open", slotAssignment: "manual" });
    return root.slotAssignment === "manual";
  })],
  ["components", "shadow root reports clonable and serializable", (w) => value(() => {
    const root = make(w, "div").attachShadow({ mode: "open", serializable: true });
    return [root.serializable, root.clonable].join(",");
  })],
  ["components", "innerHTML leaves a declarative root inert", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = '<div><template shadowrootmode="open"><i></i></template></div>';
    return host.firstElementChild?.shadowRoot ? "attached" : "template kept";
  })],
  ["components", "setHTMLUnsafe attaches a declarative root", (w) => value(() => {
    const host = make(w, "div");
    host.setHTMLUnsafe('<div><template shadowrootmode="open"><i></i></template></div>');
    return host.firstElementChild?.shadowRoot ? "attached" : "template kept";
  })],
  ["components", "customElements.define", (w) => typeof w.customElements?.define === "function"],
  ["components", "upgrade on connect", (w) => value(() => {
    const name = `probe-${Math.random().toString(36).slice(2)}`;
    let connectedCalls = 0;
    w.customElements.define(name, class extends w.HTMLElement {
      connectedCallback() { connectedCalls++; }
    });
    const element = make(w, name);
    w.document.body.append(element);
    element.remove();
    return connectedCalls === 1;
  })],
  ["components", "attributeChangedCallback", (w) => value(() => {
    const name = `probe-attr-${Math.random().toString(36).slice(2)}`;
    let changes = 0;
    w.customElements.define(name, class extends w.HTMLElement {
      static get observedAttributes() { return ["value"]; }
      attributeChangedCallback() { changes++; }
    });
    make(w, name).setAttribute("value", "one");
    return changes === 1;
  })],
  ["components", "customElements.whenDefined", (w) => typeof w.customElements?.whenDefined === "function"],
  ["components", "attachInternals", (w) => method(w, "HTMLElement", "attachInternals")],
  ["components", "form-associated custom element", (w) => value(() => {
    const name = `probe-face-${Math.random().toString(36).slice(2)}`;
    w.customElements.define(name, class extends w.HTMLElement { static formAssociated = true; });
    const internals = make(w, name).attachInternals();
    return typeof internals.setFormValue === "function";
  })],
  ["components", "CustomStateSet", (w) => global(w, "CustomStateSet")],
  ["components", "customElements.getName", (w) => value(() => {
    const name = `probe-name-${Math.random().toString(36).slice(2)}`;
    const Element = class extends w.HTMLElement {};
    w.customElements.define(name, Element);
    return typeof w.customElements.getName === "function" ? w.customElements.getName(Element) === name : "missing";
  })],
  ["components", "one constructor, one name", (w) => value(() => {
    const Element = class extends w.HTMLElement {};
    w.customElements.define(`probe-once-${Math.random().toString(36).slice(2)}`, Element);
    return errorName(() => w.customElements.define(`probe-twice-${Math.random().toString(36).slice(2)}`, Element)) ?? "defined";
  })],
  ["components", "new MyElement() from script", (w) => value(() => {
    const name = `probe-new-${Math.random().toString(36).slice(2)}`;
    const Element = class extends w.HTMLElement {};
    w.customElements.define(name, Element);
    const made = new Element();
    // Compared rather than reported: the name is random, so returning it would
    // differ between runtimes for no reason and drift on every run.
    return `${made.localName === name}/${made.isConnected}`;
  })],
  ["components", "node.assignedSlot", (w) => value(() => {
    const host = connected(w, "div");
    host.innerHTML = "<p>x</p>";
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = "<slot></slot>";
    const answer = host.firstElementChild.assignedSlot === root.firstElementChild;
    host.remove();
    return answer;
  })],
  ["components", "slotchange", async (w) => {
    const host = connected(w, "div");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = "<slot></slot>";
    let fired = 0;
    root.firstElementChild.addEventListener("slotchange", () => fired++);
    host.append(make(w, "i"));
    await new Promise((resolve) => queueMicrotask(resolve));
    host.remove();
    return fired;
  }],
  ["components", "attachShadow refuses a non-host", (w) => value(() => errorName(() => make(w, "input").attachShadow({ mode: "open" })) ?? "attached")],
  ["components", "shadowRoot.activeElement", (w) => value(() => {
    const host = connected(w, "div");
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = "<input>";
    root.firstElementChild.focus();
    const answer = `${w.document.activeElement === host}/${root.activeElement === root.firstElementChild}`;
    host.remove();
    return answer;
  })],
  ["components", ":host and ::slotted", (w) => value(() => {
    const host = connected(w, "div");
    host.innerHTML = "<p>x</p>";
    const root = host.attachShadow({ mode: "open" });
    root.innerHTML = "<style>:host { color: rgb(9, 9, 9) } ::slotted(p) { font-style: italic }</style><slot></slot>";
    const answer = `${w.getComputedStyle(host).color}/${w.getComputedStyle(host.firstElementChild).fontStyle}`;
    host.remove();
    return answer;
  })],
  ["components", "ARIA reflects", (w) => value(() => {
    const element = make(w, "div");
    element.role = "button";
    element.ariaLabel = "Save";
    return `${element.getAttribute("role")}/${element.getAttribute("aria-label")}/${make(w, "div").ariaLabel}`;
  })],
  ["components", "formDisabledCallback", (w) => value(() => {
    const name = `probe-disabled-${Math.random().toString(36).slice(2)}`;
    const log = [];
    w.customElements.define(name, class extends w.HTMLElement {
      static formAssociated = true;
      formDisabledCallback(state) { log.push(state); }
    });
    const fieldset = make(w, "fieldset");
    const element = make(w, name);
    fieldset.append(element);
    w.document.body.append(fieldset);
    fieldset.disabled = true;
    fieldset.remove();
    return log.join(",") || "none";
  })],
  ["components", "a clonable root is cloned", (w) => value(() => {
    const host = make(w, "div");
    host.attachShadow({ mode: "open", clonable: true }).innerHTML = "<i>x</i>";
    return host.cloneNode(true).shadowRoot?.innerHTML ?? "no root";
  })],
  ["components", "template content is inert", (w) => value(() => {
    const name = `probe-inert-${Math.random().toString(36).slice(2)}`;
    w.customElements.define(name, class extends w.HTMLElement {});
    const template = make(w, "template");
    template.innerHTML = `<${name}></${name}>`;
    return template.content.firstElementChild.matches(":defined");
  })],
  ["components", "template.content", (w) => value(() => {
    const template = make(w, "template");
    template.innerHTML = "<i>x</i>";
    return `${template.content?.constructor?.name ?? "none"}/${template.content?.childNodes?.length ?? -1}`;
  })],

  // --- parsing and serialization -------------------------------------------
  ["parsing", "DOMParser", (w) => global(w, "DOMParser")],
  ["parsing", "DOMParser parses text/html", (w) => value(() => {
    const parsed = new w.DOMParser().parseFromString("<p>one</p>", "text/html");
    return `${parsed.documentElement.tagName}/${parsed.body.innerHTML}`;
  })],
  ["parsing", "XMLSerializer", (w) => global(w, "XMLSerializer")],
  ["parsing", "document.write", (w) => typeof w.document.write === "function"],
  ["parsing", "malformed HTML", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<p><i>unclosed";
    return host.innerHTML;
  })],
  ["parsing", "misnested tags", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<b><i>x</b></i>";
    return host.innerHTML;
  })],
  ["parsing", "implied tbody", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<table><tr><td>x</td></tr></table>";
    return host.innerHTML;
  })],
  ["parsing", "omitted list end tags", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<ul><li>a<li>b</ul>";
    return host.innerHTML;
  })],
  ["parsing", "omitted paragraph end tag", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<p>one<p>two";
    return host.innerHTML;
  })],
  ["parsing", "fragment context element", (w) => value(() => {
    const table = make(w, "table");
    table.innerHTML = "<tr><td>x</td></tr>";
    return table.firstElementChild.localName;
  })],
  ["parsing", "SVG namespace from markup", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "<svg><circle r=\"1\"/></svg>";
    return host.firstElementChild?.namespaceURI ?? "none";
  })],
  ["parsing", "entity decoding", (w) => value(() => {
    const host = make(w, "div");
    host.innerHTML = "&amp;&nbsp;&#x41;";
    return host.textContent === "& A";
  })],

  // --- selectors -----------------------------------------------------------
  ["selectors", ":is()", (w) => value(() => w.document.querySelectorAll("div:is(.a, .b)").length >= 0)],
  ["selectors", ":where()", (w) => value(() => w.document.querySelectorAll("div:where(.a)").length >= 0)],
  ["selectors", ":has()", (w) => value(() => w.document.querySelectorAll("div:has(> i)").length >= 0)],
  ["selectors", ":nth-child(An+B of S)", (w) => value(() => w.document.querySelectorAll("li:nth-child(2n of .x)").length >= 0)],
  ["selectors", ":defined", (w) => value(() => w.document.querySelectorAll(":defined").length >= 0)],
  ["selectors", ":state()", (w) => value(() => w.document.querySelectorAll("div:state(loading)").length >= 0)],
  ["selectors", ":scope", (w) => value(() => w.document.body.querySelectorAll(":scope > *").length >= 0)],
  ["selectors", ":hover parses", (w) => value(() => w.document.querySelectorAll("a:hover").length >= 0)],
  ["selectors", ":focus-within", (w) => value(() => w.document.querySelectorAll("div:focus-within").length >= 0)],
  ["selectors", ":placeholder-shown", (w) => value(() => w.document.querySelectorAll("input:placeholder-shown").length >= 0)],
  ["selectors", ":read-write", (w) => value(() => w.document.querySelectorAll("input:read-write").length >= 0)],
  ["selectors", ":checked / :disabled", (w) => value(() => w.document.querySelectorAll("input:checked, input:disabled").length >= 0)],
  ["selectors", "invalid selector throws", (w) => value(() => {
    try {
      w.document.querySelector("div[");
      return "no throw";
    } catch (error) {
      return error.name;
    }
  })],

  // --- forms ---------------------------------------------------------------
  ["forms", "FormData", (w) => global(w, "FormData")],
  ["forms", "new FormData(form)", (w) => value(() => {
    const form = connected(w, "form");
    form.innerHTML = "<input name=a value=1>";
    const entries = [...new w.FormData(form)].length;
    form.remove();
    return entries;
  })],
  ["forms", "form.requestSubmit", (w) => method(w, "HTMLFormElement", "requestSubmit")],
  ["forms", "checkValidity", (w) => method(w, "HTMLInputElement", "checkValidity")],
  ["forms", "validity is a ValidityState", (w) => value(() => {
    const input = make(w, "input");
    input.required = true;
    return `${input.validity?.constructor?.name ?? "none"}/${input.validity?.valueMissing}/${input.validity === input.validity}`;
  })],
  ["forms", "setCustomValidity", (w) => method(w, "HTMLInputElement", "setCustomValidity")],
  ["forms", "input.valueAsNumber", (w) => accessor(w, "HTMLInputElement", "valueAsNumber")],
  ["forms", "input.valueAsDate", (w) => accessor(w, "HTMLInputElement", "valueAsDate")],
  ["forms", "setSelectionRange", (w) => method(w, "HTMLInputElement", "setSelectionRange")],
  ["forms", "labels collection", (w) => accessor(w, "HTMLInputElement", "labels")],
  ["forms", "input.indeterminate", (w) => accessor(w, "HTMLInputElement", "indeterminate")],
  ["forms", "select.selectedOptions", (w) => accessor(w, "HTMLSelectElement", "selectedOptions")],
  ["forms", "fieldset.elements", (w) => accessor(w, "HTMLFieldSetElement", "elements")],
  ["forms", "submitting dispatches submit", (w) => value(() => {
    const form = connected(w, "form");
    const button = make(w, "button");
    form.append(button);
    let submitted = 0;
    form.addEventListener("submit", (event) => { event.preventDefault(); submitted++; });
    button.click();
    form.remove();
    return submitted === 1;
  })],

  // --- styles and the cascade ----------------------------------------------
  ["cascade", "element.style", (w) => value(() => {
    const element = make(w, "div");
    element.style.color = "red";
    return element.getAttribute("style");
  })],
  ["cascade", "style.setProperty (custom property)", (w) => value(() => {
    const element = make(w, "div");
    element.style.setProperty("--x", "1px");
    return element.style.getPropertyValue("--x");
  })],
  ["cascade", "CSSStyleSheet", (w) => global(w, "CSSStyleSheet")],
  ["cascade", "constructable stylesheets", (w) => value(() => {
    const sheet = new w.CSSStyleSheet();
    sheet.replaceSync(".a { color: red }");
    return sheet.cssRules.length;
  })],
  ["cascade", "document.styleSheets", (w) => value(() => typeof w.document.styleSheets?.length === "number")],
  ["cascade", "style element has a sheet", (w) => accessor(w, "HTMLStyleElement", "sheet")],
  ["cascade", "adoptedStyleSheets applies", (w) => value(() => {
    const sheet = new w.CSSStyleSheet();
    sheet.replaceSync(".probe-adopt { color: rgb(1, 2, 3) }");
    const element = connected(w, "div");
    element.className = "probe-adopt";
    w.document.adoptedStyleSheets = [sheet];
    const colour = w.getComputedStyle(element).color;
    w.document.adoptedStyleSheets = [];
    element.remove();
    return colour;
  })],
  ["cascade", "getComputedStyle", (w) => typeof w.getComputedStyle === "function"],
  ["cascade", "cascade from a stylesheet", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = ".probe-cascade { color: rgb(1, 2, 3) }";
    const element = connected(w, "div");
    element.className = "probe-cascade";
    w.document.body.append(style);
    const colour = w.getComputedStyle(element).color;
    style.remove();
    element.remove();
    return colour;
  })],
  ["cascade", "specificity decides", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = "div { color: rgb(1, 1, 1) } .probe-spec { color: rgb(2, 2, 2) } #probe-spec { color: rgb(3, 3, 3) }";
    const element = connected(w, "div");
    element.className = "probe-spec";
    element.id = "probe-spec";
    w.document.body.append(style);
    const colour = w.getComputedStyle(element).color;
    style.remove();
    element.remove();
    return colour;
  })],
  ["cascade", "important beats inline", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = ".probe-imp { color: rgb(1, 1, 1) !important }";
    const element = connected(w, "div");
    element.className = "probe-imp";
    element.style.color = "rgb(2, 2, 2)";
    w.document.body.append(style);
    const colour = w.getComputedStyle(element).color;
    style.remove();
    element.remove();
    return colour;
  })],
  ["cascade", "inheritance", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = ".probe-inherit { color: rgb(4, 5, 6) }";
    const parent = connected(w, "div");
    parent.className = "probe-inherit";
    const child = make(w, "span");
    parent.append(child);
    w.document.body.append(style);
    const colour = w.getComputedStyle(child).color;
    style.remove();
    parent.remove();
    return colour;
  })],
  ["cascade", "user-agent display defaults", (w) => value(() => {
    const div = connected(w, "div");
    const span = connected(w, "span");
    const answer = [w.getComputedStyle(div).display, w.getComputedStyle(span).display].join(",");
    div.remove();
    span.remove();
    return answer;
  })],
  ["cascade", "no computed style outside the tree", (w) => value(() => w.getComputedStyle(make(w, "div")).length === 0)],
  ["cascade", "@media follows the viewport", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = "@media (min-width: 100px) { .probe-media { color: rgb(7, 7, 7) } } @media (min-width: 99999px) { .probe-media { color: rgb(8, 8, 8) } }";
    const element = connected(w, "div");
    element.className = "probe-media";
    w.document.body.append(style);
    const colour = w.getComputedStyle(element).color;
    style.remove();
    element.remove();
    return colour;
  })],
  ["cascade", "matchMedia answers", (w) => value(() => `${w.matchMedia("(min-width: 1px)").matches},${w.matchMedia("(min-width: 99999px)").matches}`)],
  ["cascade", "CSS.supports", (w) => value(() => `${w.CSS?.supports?.("color", "red")},${w.CSS?.supports?.("(display: grid)")}`)],
  ["cascade", "CSS nesting resolves", (w) => value(() => {
    const style = make(w, "style");
    style.textContent = ".probe-nest { color: rgb(1, 1, 1); & a { color: rgb(2, 2, 2) } }";
    const card = connected(w, "div");
    card.className = "probe-nest";
    card.innerHTML = "<a>x</a>";
    w.document.body.append(style);
    const colour = w.getComputedStyle(card.firstElementChild).color;
    style.remove();
    card.remove();
    return colour;
  })],
  ["cascade", "getBoundingClientRect measures", (w) => value(() => {
    const element = connected(w, "div");
    const rect = element.getBoundingClientRect();
    element.remove();
    if (typeof rect?.width !== "number") return "missing";
    return rect.width > 0 ? "measured" : "zero";
  })],
  ["cascade", "offsetWidth measures", (w) => value(() => {
    const element = connected(w, "div");
    if (!("offsetWidth" in element)) return "missing";
    const answer = element.offsetWidth > 0 ? "measured" : "zero";
    element.remove();
    return answer;
  })],
  ["cascade", "scrollIntoView", (w) => method(w, "Element", "scrollIntoView")],
  ["cascade", "Element.animate (WAAPI)", (w) => method(w, "Element", "animate")],
  ["cascade", "resolved font size", (w) => value(() => {
    const heading = connected(w, "h1");
    const size = w.getComputedStyle(heading).fontSize;
    heading.remove();
    return size;
  })],

  // --- window and host surface ---------------------------------------------
  ["window", "window === globalThis", (w) => value(() => w.window === w)],
  ["window", "location", (w) => value(() => typeof w.location?.href === "string")],
  ["window", "location parts are assignable", (w) => value(() => {
    const before = w.location.hash;
    try {
      w.location.hash = "#probe";
      const after = w.location.hash;
      w.location.hash = before;
      return after === "#probe";
    } catch (error) {
      return error.name;
    }
  })],
  ["window", "history.pushState", (w) => typeof w.history?.pushState === "function"],
  ["window", "localStorage", (w) => value(() => {
    w.localStorage.setItem("probe", "1");
    const read = w.localStorage.getItem("probe");
    w.localStorage.removeItem("probe");
    return read === "1";
  })],
  ["window", "document.cookie", (w) => has(w.document, "cookie")],
  ["window", "innerWidth", (w) => value(() => typeof w.innerWidth === "number")],
  ["window", "requestAnimationFrame", (w) => typeof w.requestAnimationFrame === "function"],
  ["window", "queueMicrotask", (w) => typeof w.queueMicrotask === "function"],
  ["window", "structuredClone", (w) => typeof w.structuredClone === "function"],
  ["window", "fetch", (w) => typeof w.fetch === "function"],
  ["window", "XMLHttpRequest", (w) => global(w, "XMLHttpRequest")],
  ["window", "WebSocket", (w) => global(w, "WebSocket")],
  ["window", "Worker", (w) => global(w, "Worker")],
  ["window", "Blob", (w) => global(w, "Blob")],
  ["window", "File / FileReader", (w) => global(w, "File") && global(w, "FileReader")],
  ["window", "URL.createObjectURL", (w) => typeof w.URL?.createObjectURL === "function"],
  ["window", "crypto.randomUUID", (w) => typeof w.crypto?.randomUUID === "function"],
  ["window", "performance.now", (w) => typeof w.performance?.now === "function"],
  ["window", "alert", (w) => typeof w.alert === "function"],
  ["window", "scrollTo", (w) => typeof w.scrollTo === "function"],
  ["window", "Image", (w) => global(w, "Image")],
  ["window", "canvas.getContext", (w) => value(() => {
    const canvas = make(w, "canvas");
    if (typeof canvas.getContext !== "function") return "missing";
    return canvas.getContext("2d") ? "context" : "null";
  })],
  ["window", "iframe.contentWindow", (w) => value(() => {
    const frame = connected(w, "iframe");
    const inner = frame.contentWindow;
    frame.remove();
    return inner ? "window" : "null";
  })],
  ["window", "MathMLElement", (w) => global(w, "MathMLElement")],
  ["window", "SVGElement", (w) => global(w, "SVGElement")],
  ["window", "SVGSVGElement", (w) => global(w, "SVGSVGElement")],
  ["window", "focus() sets activeElement", (w) => value(() => {
    const input = connected(w, "input");
    input.focus();
    const focused = w.document.activeElement === input;
    input.remove();
    return focused;
  })],
  ["window", "showPopover", (w) => method(w, "HTMLElement", "showPopover")],
  ["window", "dialog.showModal", (w) => method(w, "HTMLDialogElement", "showModal")],
  ["window", "document.startViewTransition", (w) => typeof w.document.startViewTransition === "function"],
];

export async function probe(window) {
  const answers = [];
  for (const [group, name, get] of features) {
    let answer;
    try {
      answer = await get(window);
    } catch (error) {
      answer = `throws:${error?.name ?? "Error"}`;
    }
    answers.push({ group, name, answer: answer === undefined ? "undefined" : answer });
  }
  return answers;
}
