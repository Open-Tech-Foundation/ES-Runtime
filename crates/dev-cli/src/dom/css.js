// Inline CSS only. This intentionally parses declarations, not a stylesheet:
// there is no cascade or computed-value engine in esdev's test DOM.

const STYLE = Symbol("esdev inline style");

function syntax(message) {
  throw new SyntaxError(`Invalid inline style: ${message}`);
}

function splitDeclarations(text) {
  const declarations = [];
  let start = 0;
  let quote = null;
  let depth = 0;
  for (let at = 0; at <= text.length; at += 1) {
    const char = text[at];
    if (quote) {
      if (char === quote) quote = null;
    } else if (char === "'" || char === '"') quote = char;
    else if (char === "(") depth += 1;
    else if (char === ")") {
      if (depth === 0) syntax("unexpected )");
      depth -= 1;
    } else if ((char === ";" || at === text.length) && depth === 0) {
      const declaration = text.slice(start, at).trim();
      if (declaration) declarations.push(declaration);
      start = at + 1;
    }
  }
  if (quote) syntax("unterminated string");
  if (depth) syntax("unterminated function");
  return declarations;
}

function parse(text) {
  const values = new Map();
  for (const declaration of splitDeclarations(String(text))) {
    const colon = declaration.indexOf(":");
    if (colon <= 0) syntax(`expected property: value in ${declaration}`);
    const name = declaration.slice(0, colon).trim();
    let value = declaration.slice(colon + 1).trim();
    if (!/^--[A-Za-z0-9_-]+$|^[A-Za-z-]+$/.test(name)) syntax(`invalid property ${name}`);
    const important = /\s*!important\s*$/i.test(value);
    if (important) value = value.replace(/\s*!important\s*$/i, "").trim();
    if (!value) continue;
    values.set(name, { value, priority: important ? "important" : "" });
  }
  return values;
}

function serialize(values) {
  return Array.from(values, ([name, entry]) => `${name}: ${entry.value}${entry.priority ? " !important" : ""};`).join(" ");
}

function kebab(name) {
  name = String(name);
  if (name === "cssFloat") return "float";
  if (name.startsWith("--")) return name;
  return name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

export function createCss({ Element }) {
  function state(element) {
    const raw = element.getAttribute("style") ?? "";
    let state = element[STYLE];
    if (!state) {
      state = { raw: null, values: new Map() };
      Object.defineProperty(element, STYLE, { value: state });
    }
    if (state.raw !== raw) {
      state.values = parse(raw);
      state.raw = raw;
    }
    return state;
  }

  function write(element, state) {
    const raw = serialize(state.values);
    state.raw = raw;
    if (raw) element.setAttribute("style", raw);
    else element.removeAttribute("style");
  }

  class CSSStyleDeclaration {
    constructor(element) {
      Object.defineProperty(this, "element", { value: element });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target.item(Number(property));
          if (typeof property === "string" && !(property in target)) return target.getPropertyValue(kebab(property));
          return Reflect.get(target, property, receiver);
        },
        set(target, property, value, receiver) {
          if (typeof property === "string" && !(property in target)) {
            target.setProperty(kebab(property), value);
            return true;
          }
          return Reflect.set(target, property, value, receiver);
        },
      });
    }
    _state() { return state(this.element); }
    get length() { return this._state().values.size; }
    item(index) { return Array.from(this._state().values.keys())[index] ?? ""; }
    getPropertyValue(name) { return this._state().values.get(String(name))?.value ?? ""; }
    getPropertyPriority(name) { return this._state().values.get(String(name))?.priority ?? ""; }
    setProperty(name, value, priority = "") {
      name = String(name).trim();
      value = value == null ? "" : String(value).trim();
      priority = String(priority).trim().toLowerCase();
      if (!/^--[A-Za-z0-9_-]+$|^[A-Za-z-]+$/.test(name)) syntax(`invalid property ${name}`);
      if (priority !== "" && priority !== "important") syntax(`invalid priority ${priority}`);
      if (!value) return this.removeProperty(name);
      const current = this._state();
      current.values.set(name, { value, priority });
      write(this.element, current);
    }
    removeProperty(name) {
      const current = this._state();
      name = String(name);
      const previous = current.values.get(name)?.value ?? "";
      current.values.delete(name);
      write(this.element, current);
      return previous;
    }
    get cssText() { return serialize(this._state().values); }
    set cssText(value) {
      const current = this._state();
      current.values = parse(String(value));
      write(this.element, current);
    }
  }

  function install() {
    Object.defineProperty(Element.prototype, "style", {
      get() {
        let style = this[STYLE]?.declaration;
        if (!style) {
          style = new CSSStyleDeclaration(this);
          state(this).declaration = style;
        }
        return style;
      },
    });
  }

  return { CSSStyleDeclaration, install };
}
