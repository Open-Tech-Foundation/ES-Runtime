// Inline CSS only. This intentionally parses declarations, not a stylesheet:
// there is no cascade or computed-value engine in esdev's test DOM.

const STYLE = Symbol("esdev inline style");

// The CSS properties this DOM recognises. A declaration list is not an open map
// in a browser — it is the IDL surface of the known properties — so an unknown
// name reads `undefined` rather than `""`, and `@supports (made-up: 1)` is
// false. Custom properties are always known, by definition.
//
// Longhands, the shorthands that expand to them, and the modern layout,
// typography and colour properties. A property missing here answers "not
// supported"; adding one is a one-line change.
const PROPERTIES = new Set(`
accent-color align-content align-items align-self all anchor-name animation animation-composition
animation-delay animation-direction animation-duration animation-fill-mode animation-iteration-count
animation-name animation-play-state animation-range animation-timeline animation-timing-function appearance
aspect-ratio backdrop-filter backface-visibility background background-attachment background-blend-mode
background-clip background-color background-image background-origin background-position background-position-x
background-position-y background-repeat background-size block-size border border-block border-block-color
border-block-end border-block-end-color border-block-end-style border-block-end-width border-block-start
border-block-start-color border-block-start-style border-block-start-width border-block-style border-block-width
border-bottom border-bottom-color border-bottom-left-radius border-bottom-right-radius border-bottom-style
border-bottom-width border-collapse border-color border-end-end-radius border-end-start-radius border-image
border-image-outset border-image-repeat border-image-slice border-image-source border-image-width border-inline
border-inline-color border-inline-end border-inline-end-color border-inline-end-style border-inline-end-width
border-inline-start border-inline-start-color border-inline-start-style border-inline-start-width
border-inline-style border-inline-width border-left border-left-color border-left-style border-left-width
border-radius border-right border-right-color border-right-style border-right-width border-spacing
border-start-end-radius border-start-start-radius border-style border-top border-top-color
border-top-left-radius border-top-right-radius border-top-style border-top-width border-width bottom
box-decoration-break box-shadow box-sizing break-after break-before break-inside caption-side caret-color
clear clip clip-path clip-rule color color-interpolation color-scheme column-count column-fill column-gap
column-rule column-rule-color column-rule-style column-rule-width column-span column-width columns contain
contain-intrinsic-block-size contain-intrinsic-height contain-intrinsic-inline-size contain-intrinsic-size
contain-intrinsic-width container container-name container-type content content-visibility counter-increment
counter-reset counter-set cursor cx cy d direction display dominant-baseline empty-cells field-sizing fill
fill-opacity fill-rule filter flex flex-basis flex-direction flex-flow flex-grow flex-shrink flex-wrap float
flood-color flood-opacity font font-family font-feature-settings font-kerning font-language-override
font-optical-sizing font-palette font-size font-size-adjust font-stretch font-style font-synthesis
font-variant font-variant-alternates font-variant-caps font-variant-east-asian font-variant-emoji
font-variant-ligatures font-variant-numeric font-variant-position font-variation-settings font-weight
forced-color-adjust gap grid grid-area grid-auto-columns grid-auto-flow grid-auto-rows grid-column
grid-column-end grid-column-start grid-row grid-row-end grid-row-start grid-template grid-template-areas
grid-template-columns grid-template-rows height hyphenate-character hyphens image-orientation image-rendering
inline-size inset inset-block inset-block-end inset-block-start inset-inline inset-inline-end
inset-inline-start isolation justify-content justify-items justify-self left letter-spacing lighting-color
line-break line-height list-style list-style-image list-style-position list-style-type margin margin-block
margin-block-end margin-block-start margin-bottom margin-inline margin-inline-end margin-inline-start
margin-left margin-right margin-top marker marker-end marker-mid marker-start mask mask-clip mask-composite
mask-image mask-mode mask-origin mask-position mask-repeat mask-size mask-type math-depth math-style
max-block-size max-height max-inline-size max-width min-block-size min-height min-inline-size min-width
mix-blend-mode object-fit object-position offset offset-anchor offset-distance offset-path offset-position
offset-rotate opacity order orphans outline outline-color outline-offset outline-style outline-width overflow
overflow-anchor overflow-block overflow-clip-margin overflow-inline overflow-wrap overflow-x overflow-y
overscroll-behavior overscroll-behavior-block overscroll-behavior-inline overscroll-behavior-x
overscroll-behavior-y padding padding-block padding-block-end padding-block-start padding-bottom padding-inline
padding-inline-end padding-inline-start padding-left padding-right padding-top page page-break-after
page-break-before page-break-inside paint-order perspective perspective-origin place-content place-items
place-self pointer-events position position-anchor position-area print-color-adjust quotes r resize right
rotate row-gap ruby-align ruby-position rx ry scale scroll-behavior scroll-margin scroll-margin-block
scroll-margin-block-end scroll-margin-block-start scroll-margin-bottom scroll-margin-inline
scroll-margin-inline-end scroll-margin-inline-start scroll-margin-left scroll-margin-right scroll-margin-top
scroll-padding scroll-padding-block scroll-padding-block-end scroll-padding-block-start scroll-padding-bottom
scroll-padding-inline scroll-padding-inline-end scroll-padding-inline-start scroll-padding-left
scroll-padding-right scroll-padding-top scroll-snap-align scroll-snap-stop scroll-snap-type scroll-timeline
scrollbar-color scrollbar-gutter scrollbar-width shape-image-threshold shape-margin shape-outside shape-rendering
stop-color stop-opacity stroke stroke-dasharray stroke-dashoffset stroke-linecap stroke-linejoin
stroke-miterlimit stroke-opacity stroke-width tab-size table-layout text-align text-align-last text-anchor
text-box text-box-edge text-box-trim text-combine-upright text-decoration text-decoration-color
text-decoration-line text-decoration-skip-ink text-decoration-style text-decoration-thickness text-emphasis
text-emphasis-color text-emphasis-position text-emphasis-style text-indent text-justify text-orientation
text-overflow text-rendering text-shadow text-size-adjust text-spacing-trim text-transform
text-underline-offset text-underline-position text-wrap text-wrap-mode text-wrap-style timeline-scope top
touch-action transform transform-box transform-origin transform-style transition transition-behavior
transition-delay transition-duration transition-property transition-timing-function translate unicode-bidi
user-select vector-effect vertical-align view-timeline view-transition-name visibility white-space
white-space-collapse widows width will-change word-break word-spacing writing-mode x y z-index zoom
`.trim().split(/\s+/));

// The CSS units, for the value check below. A dimension with any other unit is
// not a value a browser keeps.
const UNITS = new Set(`
% px em rem ex ch cap ic lh rlh vw vh vi vb vmin vmax svw svh svi svb svmin svmax
lvw lvh lvi lvb lvmin lvmax dvw dvh dvi dvb dvmin dvmax cqw cqh cqi cqb cqmin cqmax
cm mm q in pt pc deg grad rad turn s ms hz khz dpi dpcm dppx x fr
`.trim().split(/\s+/));

// The properties whose value may be a bare number. Everywhere else a number
// needs a unit — `mask-position: 23` is not a declaration a browser keeps —
// except zero, which needs none anywhere.
const NUMBER_VALUED = new Set(`
animation-iteration-count aspect-ratio border-image-outset border-image-slice border-image-width
column-count counter-increment counter-reset counter-set cx cy fill-opacity flex flex-grow flex-shrink
flood-opacity font-size-adjust font-weight grid-area grid-column grid-column-end grid-column-start grid-row
grid-row-end grid-row-start line-height math-depth opacity order orphans r rx ry scale shape-image-threshold
stop-opacity stroke-dasharray stroke-dashoffset stroke-miterlimit stroke-opacity stroke-width tab-size widows
x y z-index zoom
`.trim().split(/\s+/));

// Of those, the ones that also take a length, and the ones that also take a
// percentage — Chrome keeps neither `opacity: 2px` nor `border-image-slice:
// 2px`, and a renderer that appends "px" to a unitless value is asking exactly
// this question. Outside this family a dimension with a known unit is accepted,
// because that would need each property's grammar.
const NUMBER_WITH_LENGTH = new Set(`
border-image-outset border-image-width cx cy flex line-height r rx ry stroke-dasharray stroke-dashoffset
stroke-width tab-size x y
`.trim().split(/\s+/));

const NUMBER_WITH_PERCENT = new Set(`
border-image-slice border-image-width cx cy fill-opacity flex flood-opacity line-height opacity r rx ry scale
shape-image-threshold stop-opacity stroke-dasharray stroke-dashoffset stroke-opacity stroke-width x y zoom
`.trim().split(/\s+/));

const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i;
const DIMENSION = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?([A-Za-z%]+)$/i;

// The value's top-level components: whitespace-separated, with a function's
// arguments and a quoted string left whole. A number inside `calc()` or `rgb()`
// is that function's business, not this check's.
function components(value) {
  const found = [];
  let depth = 0;
  let quote = null;
  let start = 0;
  const push = (end) => { const text = value.slice(start, end).trim(); if (text) found.push(text); };
  for (let at = 0; at < value.length; at += 1) {
    const char = value[at];
    if (quote) { if (char === quote) quote = null; continue; }
    if (char === "'" || char === '"') quote = char;
    else if (char === "(") depth += 1;
    else if (char === ")") depth = Math.max(0, depth - 1);
    else if (depth === 0 && /[\s,]/.test(char)) { push(at); start = at + 1; }
  }
  push(value.length);
  return found;
}

// Whether a value is one this DOM keeps for that property. It is a check of
// value *types* — a known unit, a number only where a number is allowed — not
// of each property's full grammar: a browser also knows that `width` takes no
// negative length, and this does not.
export function validDeclaration(name, value) {
  const property = String(name);
  // A custom property's value is an arbitrary token sequence, by design.
  if (property.startsWith("--")) return value.trim() !== "";
  const lower = property === "cssFloat" ? "float" : property.toLowerCase();
  const numbers = NUMBER_VALUED.has(lower);
  for (const component of components(value)) {
    if (NUMBER.test(component)) {
      if (!numbers && Number(component) !== 0) return false;
      continue;
    }
    const dimension = DIMENSION.exec(component);
    if (!dimension) continue;
    const unit = dimension[1].toLowerCase();
    if (!UNITS.has(unit)) return false;
    // A property that takes a number does not necessarily take a length or a
    // percentage as well, and which it takes is recorded above.
    if (numbers && !(unit === "%" ? NUMBER_WITH_PERCENT : NUMBER_WITH_LENGTH).has(lower)) return false;
  }
  return true;
}

// A name script can ask a declaration about: a known property, a custom
// property, or the one legacy alias for `float`.
export function knownProperty(name) {
  const text = String(name);
  return text.startsWith("--") || PROPERTIES.has(text.toLowerCase()) || text === "float" || text === "cssFloat";
}

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
    // Dropped, not refused: a browser ignores a declaration whose value it
    // cannot parse and keeps the rest of the list, which is what makes one bad
    // line in a style attribute harmless.
    if (!validDeclaration(name, value)) continue;
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
          if (typeof property === "string" && !(property in target)) {
            const name = kebab(property);
            return knownProperty(name) ? target.getPropertyValue(name) : undefined;
          }
          return Reflect.get(target, property, receiver);
        },
        has(target, property) {
          if (typeof property === "string" && !(property in target)) return knownProperty(kebab(property));
          return Reflect.has(target, property);
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
      // A value the property cannot take is ignored, and the declaration that
      // was there stays: `el.style.width = "23"` changes nothing, as in a
      // browser in standards mode.
      if (!knownProperty(name) || !validDeclaration(name, value)) return;
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

  // A declaration list nothing can write: a rule's `style` and a computed
  // style. Same read surface as the live one, including camelCase access.
  class ReadOnlyStyleDeclaration {
    constructor(entries) {
      const values = new Map();
      for (const [name, value, important] of entries) {
        values.set(String(name), { value: String(value), priority: important ? "important" : "" });
      }
      Object.defineProperty(this, STYLE, { value: values });
      return new Proxy(this, {
        get(target, property, receiver) {
          if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target.item(Number(property));
          if (typeof property === "string" && !(property in target)) {
            const name = kebab(property);
            return knownProperty(name) ? target.getPropertyValue(name) : undefined;
          }
          return Reflect.get(target, property, receiver);
        },
        has(target, property) {
          if (typeof property === "string" && !(property in target)) return knownProperty(kebab(property));
          return Reflect.has(target, property);
        },
        set() { throw new TypeError("This style declaration is read-only"); },
      });
    }
    get length() { return this[STYLE].size; }
    item(index) { return Array.from(this[STYLE].keys())[index] ?? ""; }
    getPropertyValue(name) { return this[STYLE].get(String(name))?.value ?? ""; }
    getPropertyPriority(name) { return this[STYLE].get(String(name))?.priority ?? ""; }
    get cssText() { return serialize(this[STYLE]); }
    setProperty() { throw new TypeError("This style declaration is read-only"); }
    removeProperty() { throw new TypeError("This style declaration is read-only"); }
  }

  // Whether a declaration is one this DOM keeps, which is what `@supports` and
  // `CSS.supports` are asking. It is about the grammar, not about rendering.
  function supportsDeclaration(name, value) {
    if (!knownProperty(name)) return false;
    try {
      return parse(`${name}: ${value}`).size === 1;
    } catch {
      return false;
    }
  }

  // The same question the cascade asks of a stylesheet's declarations, which
  // arrive already split into name and value.
  function keepsDeclaration(name, value) {
    return knownProperty(name) && String(value).trim() !== "" && validDeclaration(name, String(value));
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

  return {
    CSSStyleDeclaration,
    readOnlyDeclaration: (entries) => new ReadOnlyStyleDeclaration(entries),
    supportsDeclaration,
    keepsDeclaration,
    install,
  };
}
