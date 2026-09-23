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

// Which values a property takes, and what a zero serializes as — read from the
// table `tsr gen:css-table` generates out of Chrome (see
// `wpt/dom-matrix/css-table.js`). Hand-written guesses at this were wrong about
// `cx`, then about `transition-duration`; the table is not a guess.
const UNIT_KINDS = new Map(Object.entries({
  "%": "percentage",
  s: "time", ms: "time",
  deg: "angle", grad: "angle", rad: "angle", turn: "angle",
  dpi: "resolution", dpcm: "resolution", dppx: "resolution", x: "resolution",
  hz: "frequency", khz: "frequency",
  fr: "flex",
}));

// Everything else with a unit is a length. Listing them is how an unknown unit —
// `5foo` — stays invalid.
const LENGTH_UNITS = new Set(`
px em rem ex ch cap ic lh rlh vw vh vi vb vmin vmax svw svh svi svb svmin svmax
lvw lvh lvi lvb lvmin lvmax dvw dvh dvi dvb dvmin dvmax cqw cqh cqi cqb cqmin cqmax
cm mm q in pt pc
`.trim().split(/\s+/));

function unitKind(unit) {
  const lower = unit.toLowerCase();
  return UNIT_KINDS.get(lower) ?? (LENGTH_UNITS.has(lower) ? "length" : null);
}

// The table as a lookup: property → { kinds: Set, negative: Set, zero: string }.
export function valueTypes(rows) {
  const types = new Map();
  for (const [property, accepts, zero] of rows) {
    const kinds = new Set();
    const negative = new Set();
    for (const entry of accepts.split(" ").filter(Boolean)) {
      const signed = entry.endsWith("±");
      const kind = signed ? entry.slice(0, -1) : entry;
      kinds.add(kind);
      if (signed) negative.add(kind);
    }
    types.set(property, { kinds, negative, zero });
  }
  return types;
}

const NUMBER = /^[+-]?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i;
const DIMENSION = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?([A-Za-z%]+)$/i;

// The value's top-level components: whitespace-separated, with a function's
// arguments and a quoted string left whole. A number inside `calc()` or `rgb()`
// is that function's business, not this check's.
function components(value, separators = /[\s,]/) {
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
    else if (depth === 0 && separators.test(char)) { push(at); start = at + 1; }
  }
  push(value.length);
  return found;
}

// What kind a single component is, or null when it is a keyword, a function or a
// string — something this check has nothing to say about.
function componentKind(component) {
  if (NUMBER.test(component)) return { kind: "number", negative: component.startsWith("-"), zero: Number(component) === 0 };
  const dimension = DIMENSION.exec(component);
  if (!dimension) return null;
  const kind = unitKind(dimension[1]);
  // A unit no CSS property uses is not a value any property takes.
  if (kind === null) return { kind: "unknown", negative: false, zero: false };
  return { kind, negative: component.startsWith("-"), zero: Number.parseFloat(component) === 0 };
}

// Whether a value is one this DOM keeps for that property, by the kinds the
// table says the property takes. It is a check of value *kinds*, not of each
// property's whole grammar: how many components a property takes, and in what
// order, is still beyond it.
export function validDeclaration(types, name, value) {
  const property = String(name);
  // A custom property's value is an arbitrary token sequence, by design.
  if (property.startsWith("--")) return String(value).trim() !== "";
  const lower = property === "cssFloat" ? "float" : property.toLowerCase();
  // A shorthand is checked through its longhands, because a kind may only be
  // legal in one position — `border-image` takes a length only as its width, and
  // `offset` an angle only as its rotation. An invalid part invalidates the
  // whole declaration, as it does in a browser.
  if (isShorthand(lower)) {
    const parts = expandShorthand(lower, value);
    return parts.length === 0 || parts.every(([longhand, part]) => validDeclaration(types, longhand, part));
  }
  const declared = types?.get(lower);
  for (const component of components(String(value), /\s/)) {
    const found = componentKind(component);
    if (found === null) continue;
    if (found.kind === "unknown") return false;
    // With no table — a unit test building the module on its own — only the
    // unknown-unit check applies.
    if (!declared) continue;
    // A bare zero is a length that needs no unit — `box-shadow: 0 0 2px red` —
    // but it is not a time or an angle, which is why `transition-duration: 0`
    // and `rotate: 0` are values a browser drops.
    if (found.kind === "number" && found.zero) {
      if (!declared.kinds.has("number") && !declared.kinds.has("length")) return false;
      continue;
    }
    if (!declared.kinds.has(found.kind)) return false;
    if (found.negative && !declared.negative.has(found.kind)) return false;
  }
  return true;
}

// What a declaration stores for a zero: the form Chrome keeps, which is the
// table's own answer for a single `0`, and per component otherwise — `0px` where
// the property takes a length, `0` where it takes a number.
export function normalizeZeros(types, name, value) {
  const property = String(name).toLowerCase();
  if (property.startsWith("--")) return value;
  const declared = types?.get(property);
  if (!declared) return value;
  const text = String(value).trim();
  if (NUMBER.test(text) && Number(text) === 0 && declared.zero !== "") return declared.zero;
  if (!/(^|[\s,(])[+-]?0(\.0+)?([\s,)]|$)/.test(text)) return value;
  const unit = declared.kinds.has("length") ? "0px" : declared.kinds.has("number") ? "0" : null;
  if (unit === null) return value;
  return components(text, /\s/)
    .map((component) => (NUMBER.test(component) && Number(component) === 0 ? unit : component))
    .join(" ");
}

// What a declaration stores: a colour in the canonical form a browser keeps it
// in, and a zero in the form the table says. Both come from the realm, passed in,
// so this file stays testable on its own.
function canonical(realm, name, value) {
  const { colors = null, types = null } = realm ?? {};
  const property = String(name).toLowerCase();
  if (colors?.COLOR_PROPERTIES.has(property)) return colors.specifiedColor(value);
  if (property === "box-shadow" || property === "text-shadow") {
    return canonicalShadow(normalizeZeros(types, property, quoteArguments(value)));
  }
  return normalizeZeros(types, property, quoteArguments(value));
}

// `box-shadow` and `text-shadow` print the colour first, then the offsets, then
// `inset` — whatever order they were written in.
function canonicalShadow(value) {
  return layers(String(value))
    .map((layer) => {
      const tokens = components(layer, /\s/);
      const lengths = [];
      const colours = [];
      let inset = null;
      for (const token of tokens) {
        if (token.toLowerCase() === "inset") inset = token;
        else if (isLength(token)) lengths.push(token);
        else colours.push(token);
      }
      return [...colours, ...lengths, ...(inset === null ? [] : [inset])].join(" ");
    })
    .join(", ");
}

// `url(x.svg)` is stored as `url("x.svg")`, and so is a `path()`'s string: a
// browser quotes both however they were written.
function quoteArguments(value) {
  return String(value).replace(/\b(url|path)\(\s*("[^"]*"|'[^']*'|[^)]*)\s*\)/gi, (whole, name, argument) => {
    const inner = /^["']/.test(argument) ? argument.slice(1, -1) : argument;
    return `${name.toLowerCase()}("${inner}")`;
  });
}

// Storing one declaration: a shorthand becomes its longhands, in the order the
// family expands, and replaces whatever they held. A property with no expansion
// — a longhand, a custom property — is stored as itself.
function setLonghands(values, realm, name, value, priority) {
  const parts = expandShorthand(name, value);
  if (parts.length === 0) {
    values.set(name, { value, priority });
    return;
  }
  for (const [longhand] of parts) values.delete(longhand);
  for (const [longhand, part] of parts) values.set(longhand, { value: part, priority });
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

// The realm's value table and colour helpers travel together: everything that
// decides what a declaration keeps needs both.
function parse(text, realm) {
  const { colors = null, types = null } = realm ?? {};
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
    if (!validDeclaration(types, name, value)) continue;
    setLonghands(values, realm, name, canonical(realm, name, value), important ? "important" : "");
  }
  return values;
}

// Kept for the read-only declaration a computed style hands out, which holds
// longhands and prints them one by one.
function serialize(values) {
  return Array.from(values, ([name, entry]) => `${name}: ${entry.value}${entry.priority ? " !important" : ""};`).join(" ");
}

function kebab(name) {
  name = String(name);
  if (name === "cssFloat") return "float";
  if (name.startsWith("--")) return name;
  return name.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
}

// Serializing a shorthand out of the longhands a declaration holds, which is how
// `getComputedStyle(el).border` answers `1px solid rgb(255, 0, 0)` — a computed
// style has no shorthands in it, only the parts.
//
// The families whose serialization is mechanical are listed here; a shorthand
// that is not is answered with `""`, as a browser answers one it cannot
// represent.
const EDGE_FAMILIES = new Set([
  "margin", "padding", "inset", "scroll-margin", "scroll-padding",
  "border-width", "border-style", "border-color", "border-radius",
]);

// The per-family initial values a serialization leaves out.
const OMITTED = new Map(Object.entries({
  "border-width": "medium", "border-style": "none", "border-color": "currentcolor",
  "outline-width": "medium", "outline-style": "none", "outline-color": "currentcolor",
  "text-decoration-style": "solid", "text-decoration-color": "currentcolor",
  "text-decoration-thickness": "auto", "list-style-position": "outside",
  "list-style-image": "none", "list-style-type": "disc",
  "background-image": "none", "background-position-x": "0%", "background-position-y": "0%",
  "background-size": "auto", "background-repeat": "repeat", "background-attachment": "scroll",
  "background-origin": "padding-box", "background-clip": "border-box",
  "background-color": "rgba(0, 0, 0, 0)",
  "border-image-source": "none", "border-image-slice": "100%", "border-image-width": "1",
  "border-image-outset": "0", "border-image-repeat": "stretch",
  "font-style": "normal", "font-variant": "normal", "font-weight": "normal", "font-stretch": "normal",
  "line-height": "normal",
  "mask-image": "none", "mask-position": "0% 0%", "mask-size": "auto", "mask-repeat": "repeat",
  "mask-origin": "border-box", "mask-clip": "border-box", "mask-composite": "add",
  "mask-mode": "match-source",
  "offset-position": "normal", "offset-path": "none", "offset-distance": "0", "offset-rotate": "auto",
  "offset-anchor": "auto",
}));

// The order each family prints in, which is the family's own and not the order
// its longhands expand in — `outline` prints colour, style, width where `border`
// prints width, style, colour. Read off Chrome, family by family. A `/` is
// printed where the grammar has one.
const PRINT_ORDER = {
  outline: ["outline-color", "outline-style", "outline-width"],
  "list-style": ["list-style-position", "list-style-image", "list-style-type"],
  "text-decoration": ["text-decoration-line", "text-decoration-thickness", "text-decoration-style", "text-decoration-color"],
  font: ["font-style", "font-variant", "font-weight", "font-stretch", "font-size", ["/", "line-height"], "font-family"],
  background: [
    "background-image", "background-position-x", "background-position-y", ["/", "background-size"],
    "background-repeat", "background-attachment", "background-origin", "background-clip",
    "background-color",
  ],
  "border-image": [
    "border-image-source", "border-image-slice", ["/", "border-image-width"],
    ["/", "border-image-outset"], "border-image-repeat",
  ],
  mask: [
    "mask-image", "mask-position", ["/", "mask-size"], "mask-repeat", "mask-origin", "mask-clip",
    "mask-composite", "mask-mode",
  ],
  offset: ["offset-position", "offset-path", "offset-distance", "offset-rotate", ["/", "offset-anchor"]],
};

// A run of equal values collapses, the way the 1-to-4 value pattern does.
function collapse(values) {
  const [top, right, bottom, left] = values;
  if (left === right && bottom === top && right === top) return top;
  if (left === right && bottom === top) return `${top} ${right}`;
  if (left === right) return `${top} ${right} ${bottom}`;
  return values.join(" ");
}

function serializeShorthandFrom(shorthand, read) {
  const property = String(shorthand).toLowerCase();
  const names = shorthandLonghands(property);
  if (names.length === 0) return "";
  const values = names.map((name) => read(name));
  if (values.some((value) => value === "" || value === undefined)) return "";
  if (EDGE_FAMILIES.has(property)) return collapse(values);
  // `border`, `border-top`, `outline`: the three parts in order, and only where
  // each side agrees with the others.
  if (property === "border") {
    const sides = ["top", "right", "bottom", "left"];
    for (const part of ["width", "style", "color"]) {
      const answers = sides.map((side) => read(`border-${side}-${part}`));
      if (answers.some((answer) => answer !== answers[0])) return "";
    }
    return serializeShorthandFrom("border-top", read);
  }
  const order = PRINT_ORDER[property];
  if (order !== undefined) {
    // Every part at the CSS-wide `initial` prints as that one word, which is how
    // a declaration that can no longer say `border` still says
    // `border-image: initial`.
    if (values.every((value) => value === "initial")) return "initial";
    const printed = [];
    for (const item of order) {
      const slash = Array.isArray(item);
      const name = slash ? item[1] : item;
      const value = read(name);
      if (value === "" || value === "initial" || value === OMITTED.get(name)) continue;
      printed.push(slash ? `/ ${value}` : value);
    }
    if (printed.length === 0) return OMITTED.get(Array.isArray(order[0]) ? order[0][1] : order[0]) ?? "";
    return printed.join(" ");
  }
  if (/^(border-(top|right|bottom|left)|columns|flex-flow)$/.test(property)) {
    const printed = names
      .map((name) => [name, read(name)])
      .filter(([name, value]) => value !== OMITTED.get(name))
      .map(([, value]) => value);
    return printed.length === 0 ? OMITTED.get(names[0]) ?? "" : printed.join(" ");
  }
  // The layered families: each layer serialized on its own and joined with
  // commas, the way a browser prints them. `animation` prints every part, in
  // that order, with the name last; `transition` omits a part at its default.
  if (property === "transition" || property === "animation") {
    const names = shorthandLonghands(property);
    const parts = names.map((name) => read(name).split(",").map((piece) => piece.trim()));
    const count = Math.max(...parts.map((list) => list.length));
    const defaults = property === "transition"
      ? { "transition-property": "all", "transition-timing-function": "ease", "transition-delay": "0s", "transition-behavior": "normal" }
      : {};
    const printed = [];
    for (let layer = 0; layer < count; layer += 1) {
      const pieces = [];
      const order = property === "animation"
        ? ["animation-duration", "animation-timing-function", "animation-delay", "animation-iteration-count",
           "animation-direction", "animation-fill-mode", "animation-play-state", "animation-name"]
        : names;
      for (const name of order) {
        const value = parts[names.indexOf(name)]?.[layer] ?? parts[names.indexOf(name)]?.[0];
        if (value === undefined || value === "") continue;
        if (defaults[name] === value) continue;
        pieces.push(value);
      }
      printed.push(pieces.join(" "));
    }
    return printed.join(", ");
  }
  // `grid-template`: the area strings interleaved with their row sizes, then the
  // columns after a slash.
  if (property === "grid-template") {
    const rows = read("grid-template-rows");
    const columns = read("grid-template-columns");
    const areas = read("grid-template-areas");
    if (areas === "none" || areas === "") {
      if (rows === "none" && columns === "none") return "none";
      return `${rows} / ${columns}`;
    }
    const strings = areas.match(/"[^"]*"/g) ?? [];
    const sizes = components(rows, /\s/);
    const interleaved = strings.map((string, at) => `${string} ${sizes[at] ?? "auto"}`).join(" ");
    return `${interleaved} / ${columns}`;
  }
  if (property === "flex") return values.join(" ");
  if (property === "gap" || property === "overflow" || property.startsWith("place-")
      || property === "overscroll-behavior" || /^(margin|padding|inset)-(block|inline)$/.test(property)) {
    return values[0] === values[1] ? values[0] : values.join(" ");
  }
  // The axes always print as a pair, even when they agree: `0px center`.
  if (property === "background-position") return values.join(" ");
  if (/^grid-(row|column)$/.test(property)) {
    return values[0] === values[1] ? values[0] : values.join(" / ");
  }
  if (property === "grid-area") {
    const [rowStart, columnStart, rowEnd, columnEnd] = values;
    return rowEnd === rowStart && columnEnd === columnStart
      ? `${rowStart} / ${columnStart}`
      : `${rowStart} / ${columnStart} / ${rowEnd} / ${columnEnd}`;
  }
  return "";
}

// Which shorthands cover a longhand, largest first — the order a declaration
// block is serialized in, so `border` is tried before `border-width` and
// `border-width` before `border-top`.
// Built on first use: the shorthand table is declared further down the file, and
// this index is over it.
let covering = null;
function coveringShorthands(longhand) {
  if (covering === null) {
    covering = new Map();
    for (const shorthand of SHORTHANDS.keys()) {
      const names = shorthandLonghands(shorthand);
      for (const name of names) {
        if (!covering.has(name)) covering.set(name, []);
        covering.get(name).push([shorthand, names]);
      }
    }
    for (const list of covering.values()) list.sort(([, left], [, right]) => right.length - left.length);
  }
  return covering.get(longhand) ?? [];
}

// "Serialize a CSS declaration block": walk the longhands in order and print the
// largest shorthand each one belongs to whose whole family is present at the same
// importance, falling back to the longhand itself. This is why setting all four
// margins prints `margin: 1px 2px;` and why overriding one border width prints
// `border-width: 9px 1px 1px; border-style: solid; …` instead of `border`.
function serializeBlock(values) {
  const consumed = new Set();
  const out = [];
  // Normal declarations first, then the important ones, each group in the order
  // it was written — which is the order a browser prints a block in.
  const ordered = [
    ...Array.from(values).filter(([, entry]) => entry.priority !== "important"),
    ...Array.from(values).filter(([, entry]) => entry.priority === "important"),
  ];
  for (const [name, entry] of ordered) {
    if (consumed.has(name)) continue;
    let printed = false;
    for (const [shorthand, names] of coveringShorthands(name)) {
      if (names.some((longhand) => {
        const part = values.get(longhand);
        return part === undefined || part.priority !== entry.priority;
      })) continue;
      const text = serializeShorthandFrom(shorthand, (longhand) => values.get(longhand)?.value ?? "");
      if (text === "") continue;
      out.push(`${shorthand}: ${text}${entry.priority ? " !important" : ""};`);
      for (const longhand of names) consumed.add(longhand);
      printed = true;
      break;
    }
    if (printed) continue;
    out.push(`${name}: ${entry.value}${entry.priority ? " !important" : ""};`);
    consumed.add(name);
  }
  return out.join(" ");
}

export function createCss({ Element, colors = null, valueTable = null }) {
  const types = valueTable === null ? null : valueTypes(valueTable);
  const realm = { colors, types };
  function state(element) {
    const raw = element.getAttribute("style") ?? "";
    let state = element[STYLE];
    if (!state) {
      state = { raw: null, values: new Map() };
      Object.defineProperty(element, STYLE, { value: state });
    }
    if (state.raw !== raw) {
      state.values = parse(raw, realm);
      state.raw = raw;
    }
    return state;
  }

  function write(element, state) {
    // The attribute gets the block serialization, so a shorthand written through
    // the object comes back out of the attribute as a shorthand.
    const raw = serializeBlock(state.values);
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
    getPropertyValue(name) {
      const property = String(name);
      const values = this._state().values;
      const own = values.get(property);
      if (own) return own.value;
      // The declaration holds longhands; a shorthand is assembled from them, and
      // answers `""` when the family is incomplete — as a browser answers one it
      // cannot represent.
      return serializeShorthandFrom(property, (longhand) => values.get(longhand)?.value ?? "");
    }
    getPropertyPriority(name) {
      const property = String(name);
      const values = this._state().values;
      const own = values.get(property);
      if (own) return own.priority;
      const names = shorthandLonghands(property);
      if (names.length === 0) return "";
      return names.every((longhand) => values.get(longhand)?.priority === "important") ? "important" : "";
    }
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
      if (!knownProperty(name) || !validDeclaration(types, name, value)) return;
      const current = this._state();
      setLonghands(current.values, realm, name, canonical(realm, name, value), priority);
      write(this.element, current);
    }
    removeProperty(name) {
      const current = this._state();
      name = String(name);
      const previous = this.getPropertyValue(name);
      // A shorthand takes its whole family with it.
      const names = shorthandLonghands(name);
      if (names.length > 0) for (const longhand of names) current.values.delete(longhand);
      else current.values.delete(name);
      write(this.element, current);
      return previous;
    }
    get cssText() { return serializeBlock(this._state().values); }
    set cssText(value) {
      const current = this._state();
      current.values = parse(String(value), realm);
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
    getPropertyValue(name) {
      const property = String(name);
      const own = this[STYLE].get(property);
      if (own) return own.value;
      // A computed style holds longhands; a shorthand asked of it is serialized
      // back out of them.
      return serializeShorthandFrom(property, (longhand) => this[STYLE].get(longhand)?.value ?? "");
    }
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
      return parse(`${name}: ${value}`, realm).size >= 1;
    } catch {
      return false;
    }
  }

  // The same question the cascade asks of a stylesheet's declarations, which
  // arrive already split into name and value.
  function keepsDeclaration(name, value) {
    return knownProperty(name) && String(value).trim() !== "" && validDeclaration(types, name, String(value));
  }

  function install() {
    Object.defineProperty(Element.prototype, "style", { configurable: true, enumerable: true,
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

  // A declaration iterates its property names, as any list with an indexed
  // getter does: through Array's own `values`.
  CSSStyleDeclaration.prototype[Symbol.iterator] = Array.prototype.values;
  ReadOnlyStyleDeclaration.prototype[Symbol.iterator] = Array.prototype.values;

  return {
    CSSStyleDeclaration,
    readOnlyDeclaration: (entries) => new ReadOnlyStyleDeclaration(entries),
    supportsDeclaration,
    keepsDeclaration,
    // The realm's value table, so the cascade can ask the same questions the
    // declaration does — which properties take a length, above all.
    types,
    expandShorthand,
    shorthandLonghands,
    install,
  };
}

// ---------------------------------------------------------------------------
// Shorthands
//
// A browser's computed style has no shorthands in it: `border: 2px solid blue`
// is twelve longhands by the time anything reads `border-top-width`, and a
// component test that sets a shorthand and asserts a longhand — which is most
// of them — depends on that. The table below expands the families whose
// grammar is decidable from the tokens themselves. `transition`, `animation`,
// `mask`, `offset` and `grid-template` are deliberately absent: their values
// are comma-separated lists whose parts need each property's own grammar, and a
// wrong expansion is worse than none.
// ---------------------------------------------------------------------------

const SIDES = ["top", "right", "bottom", "left"];
const BORDER_IMAGE = [
  "border-image-source", "border-image-slice", "border-image-width", "border-image-outset",
  "border-image-repeat",
];
const CORNERS = ["top-left", "top-right", "bottom-right", "bottom-left"];
const BORDER_STYLES = new Set([
  "none", "hidden", "dotted", "dashed", "solid", "double", "groove", "ridge", "inset", "outset",
]);
const BORDER_WIDTHS = new Set(["thin", "medium", "thick"]);
const GLOBALS = new Set(["inherit", "initial", "unset", "revert", "revert-layer"]);
const FONT_STYLES = new Set(["normal", "italic", "oblique"]);
const FONT_VARIANTS = new Set(["normal", "small-caps"]);
const FONT_WEIGHTS = new Set(["normal", "bold", "bolder", "lighter"]);
const FONT_STRETCHES = new Set([
  "normal", "ultra-condensed", "extra-condensed", "condensed", "semi-condensed",
  "semi-expanded", "expanded", "extra-expanded", "ultra-expanded",
]);
const BACKGROUND_REPEATS = new Set(["repeat", "repeat-x", "repeat-y", "no-repeat", "space", "round"]);
const BACKGROUND_ATTACHMENTS = new Set(["scroll", "fixed", "local"]);
const BOXES = new Set(["border-box", "padding-box", "content-box", "text"]);
const POSITIONS = new Set(["left", "right", "top", "bottom", "center"]);
const LIST_POSITIONS = new Set(["inside", "outside"]);

const isLength = (token) => DIMENSION.test(token) || NUMBER.test(token) || token.startsWith("calc(");
const isImage = (token) => /^(url|linear-gradient|radial-gradient|conic-gradient|repeating-|image-set|-webkit-)/.test(token);

// One value for each of several longhands.
const each = (names, value) => names.map((name) => [name, value]);

// The 1-to-4 value pattern: one value for all, two for the axes, three with the
// sides' middle repeated, four in order.
function edges(values) {
  const [first, second = first, third = first, fourth = second] = values;
  return [first, second, third, fourth];
}

// `border`, `border-top`, `outline`: width, style and colour in any order, told
// apart by what each token is. Anything that is neither a style nor a width is
// the colour, which is how a browser can accept `currentcolor`, a hex, a
// function or a name here without a table of names.
function lineParts(values) {
  const parts = { width: null, style: null, color: null };
  for (const token of values) {
    const lower = token.toLowerCase();
    if (parts.style === null && BORDER_STYLES.has(lower)) parts.style = token;
    else if (parts.width === null && (BORDER_WIDTHS.has(lower) || isLength(token))) parts.width = token;
    else if (parts.color === null) parts.color = token;
  }
  return parts;
}

function line(prefix, values) {
  const { width, style, color } = lineParts(values);
  return [
    [`${prefix}-width`, width ?? "medium"],
    [`${prefix}-style`, style ?? "none"],
    [`${prefix}-color`, color ?? "currentcolor"],
  ];
}

// `border-radius`, whose two groups are the horizontal and vertical radii:
// `10px 20px / 5px` is a corner of `10px 5px`.
function radii(values) {
  const slash = values.indexOf("/");
  const horizontal = edges(slash === -1 ? values : values.slice(0, slash));
  const vertical = slash === -1 ? null : edges(values.slice(slash + 1));
  return CORNERS.map((corner, index) => [
    `border-${corner}-radius`,
    vertical === null ? horizontal[index] : `${horizontal[index]} ${vertical[index]}`,
  ]);
}

// `font: italic small-caps bold 12px/1.5 serif`. The size is the first token
// that is a length or a font-size keyword; everything before it is one of the
// four optional keywords, everything after it is the family.
const FONT_SIZES = new Set([
  "xx-small", "x-small", "small", "medium", "large", "x-large", "xx-large", "xxx-large", "larger", "smaller",
]);
function font(values) {
  const at = values.findIndex((token) => {
    const [size] = token.split("/");
    return FONT_SIZES.has(size.toLowerCase()) || isLength(size);
  });
  if (at === -1) return [];
  const [size, height] = values[at].split("/");
  const out = [["font-size", size], ["font-line-height-placeholder", null]];
  out.pop();
  out.push(["line-height", height ?? "normal"]);
  const found = { "font-style": "normal", "font-variant": "normal", "font-weight": "normal", "font-stretch": "normal" };
  for (const token of values.slice(0, at)) {
    const lower = token.toLowerCase();
    if (FONT_STYLES.has(lower) && lower !== "normal") found["font-style"] = token;
    else if (FONT_VARIANTS.has(lower) && lower !== "normal") found["font-variant"] = token;
    else if (FONT_WEIGHTS.has(lower) || NUMBER.test(token)) found["font-weight"] = token;
    else if (FONT_STRETCHES.has(lower) && lower !== "normal") found["font-stretch"] = token;
  }
  const family = values.slice(at + 1).join(" ");
  if (family) out.push(["font-family", family]);
  return [...out, ...Object.entries(found)];
}

// `background`, by the same "what is this token" reading. The part after a
// slash is the size, because that is the only place one can appear.
function background(values) {
  const found = {
    "background-image": "none", "background-repeat": "repeat", "background-attachment": "scroll",
    "background-position": "0% 0%", "background-size": "auto", "background-color": "rgba(0, 0, 0, 0)",
    "background-origin": "padding-box", "background-clip": "border-box",
  };
  const position = [];
  const size = [];
  const boxes = [];
  let afterSlash = false;
  const isSize = (token) => ["auto", "cover", "contain"].includes(token.toLowerCase()) || isLength(token);
  for (const token of values) {
    if (token === "/") { afterSlash = true; continue; }
    const lower = token.toLowerCase();
    if (afterSlash && isSize(token)) { size.push(token); continue; }
    afterSlash = false;
    if (isImage(lower)) found["background-image"] = token;
    else if (BACKGROUND_REPEATS.has(lower)) found["background-repeat"] = token;
    else if (BACKGROUND_ATTACHMENTS.has(lower)) found["background-attachment"] = token;
    else if (BOXES.has(lower)) boxes.push(token);
    else if (POSITIONS.has(lower) || isLength(token)) position.push(token);
    else found["background-color"] = token;
  }
  if (position.length) found["background-position"] = position.length === 1 ? `${position[0]} center` : position.join(" ");
  // A browser's declaration holds the two axes, not the pair.
  const [positionX, positionY] = components(found["background-position"], /\s/);
  delete found["background-position"];
  found["background-position-x"] = positionX;
  found["background-position-y"] = positionY ?? "center";
  if (size.length) found["background-size"] = size.join(" ");
  if (boxes.length) {
    found["background-origin"] = boxes[0];
    found["background-clip"] = boxes[1] ?? boxes[0];
  }
  // In the order a browser enumerates them, which is the family's grammar order
  // rather than the order this code fills them in.
  return [
    "background-image", "background-position-x", "background-position-y", "background-size",
    "background-repeat", "background-attachment", "background-origin", "background-clip",
    "background-color",
  ].map((name) => [name, found[name]]);
}

// `flex: 1` is `1 1 0%`, `flex: auto` is `1 1 auto`, `flex: none` is `0 0 auto`
// — the three the specification spells out, because they are not what reading
// the tokens in order would give.
function flex(values) {
  const joined = values.join(" ").toLowerCase();
  if (joined === "none") return [["flex-grow", "0"], ["flex-shrink", "0"], ["flex-basis", "auto"]];
  if (joined === "auto") return [["flex-grow", "1"], ["flex-shrink", "1"], ["flex-basis", "auto"]];
  const numbers = values.filter((token) => NUMBER.test(token));
  const rest = values.filter((token) => !NUMBER.test(token));
  return [
    ["flex-grow", numbers[0] ?? "1"],
    ["flex-shrink", numbers[1] ?? "1"],
    ["flex-basis", rest[0] ?? (numbers.length ? "0%" : "auto")],
  ];
}

// Split on the slash a two-ended shorthand uses: `grid-row: 1 / 3`.
function ends(names, values, initial) {
  const slash = values.indexOf("/");
  const parts = slash === -1
    ? [values.join(" ")]
    : [values.slice(0, slash).join(" "), values.slice(slash + 1).join(" ")];
  return names.map((name, index) => [name, parts[index] ?? parts[0] ?? initial]);
}

// The comma-separated families. Each layer is read on its own and the answers
// are joined back with commas, which is how a browser reports them: `transition:
// opacity 2s ease-in 1s, color 3s` gives `transition-duration: 2s, 3s`.
function layers(text) {
  const found = [];
  let depth = 0;
  let quote = null;
  let start = 0;
  for (let at = 0; at < text.length; at += 1) {
    const char = text[at];
    if (quote) { if (char === quote) quote = null; continue; }
    if (char === "'" || char === '"') quote = char;
    else if (char === "(") depth += 1;
    else if (char === ")") depth -= 1;
    else if (char === "," && depth === 0) { found.push(text.slice(start, at).trim()); start = at + 1; }
  }
  found.push(text.slice(start).trim());
  return found.filter(Boolean);
}

const TIMING_KEYWORDS = new Set(["ease", "linear", "ease-in", "ease-out", "ease-in-out", "step-start", "step-end"]);
const isTiming = (token) => TIMING_KEYWORDS.has(token.toLowerCase()) || /^(cubic-bezier|steps|linear)\(/i.test(token);
const isTime = (token) => /^[+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?(s|ms)$/i.test(token);

// One layer of a comma-separated family: the answers for each longhand, by
// reading each token for what it can be. A time is the duration the first time
// and the delay the second, which is the only positional rule in the family.
function perLayer(names, read) {
  return (parts, text) => {
    const found = layers(text).map((layer) => read(components(layer, /\s/)));
    return names.map((name, index) => [name, found.map((answers) => answers[index]).join(", ")]);
  };
}

const transition = perLayer(
  ["transition-property", "transition-duration", "transition-timing-function", "transition-delay", "transition-behavior"],
  (tokens) => {
    const answers = ["all", "0s", "ease", "0s", "normal"];
    let times = 0;
    for (const token of tokens) {
      const lower = token.toLowerCase();
      if (isTime(token)) { answers[times === 0 ? 1 : 3] = token; times += 1; }
      else if (isTiming(token)) answers[2] = token;
      else if (lower === "normal" || lower === "allow-discrete") answers[4] = token;
      else answers[0] = token;
    }
    return answers;
  },
);

const ANIMATION_DIRECTIONS = new Set(["normal", "reverse", "alternate", "alternate-reverse"]);
const ANIMATION_FILLS = new Set(["none", "forwards", "backwards", "both"]);
const ANIMATION_STATES = new Set(["running", "paused"]);

const animation = perLayer(
  ["animation-name", "animation-duration", "animation-timing-function", "animation-delay",
   "animation-iteration-count", "animation-direction", "animation-fill-mode", "animation-play-state"],
  (tokens) => {
    const answers = ["none", "0s", "ease", "0s", "1", "normal", "none", "running"];
    let times = 0;
    let named = false;
    for (const token of tokens) {
      const lower = token.toLowerCase();
      if (isTime(token)) { answers[times === 0 ? 1 : 3] = token; times += 1; }
      else if (isTiming(token)) answers[2] = token;
      else if (lower === "infinite" || NUMBER.test(token)) answers[4] = token;
      else if (ANIMATION_DIRECTIONS.has(lower) && answers[5] === "normal" && lower !== "normal") answers[5] = token;
      // `none` is a name as much as it is a fill mode, and the name wins while
      // the name is still unset — which is the order a browser reads them in.
      else if (ANIMATION_FILLS.has(lower) && !(lower === "none" && !named) && answers[6] === "none") answers[6] = token;
      else if (ANIMATION_STATES.has(lower) && lower !== "running") answers[7] = token;
      else if (lower === "running" && answers[7] === "running" && named) answers[7] = token;
      else { answers[0] = token; named = true; }
    }
    return answers;
  },
);

// `grid-template`, whose left side may interleave area strings with row sizes.
function gridTemplate(parts, text) {
  const names = ["grid-template-rows", "grid-template-columns", "grid-template-areas"];
  const slash = text.indexOf("/");
  const rowsText = (slash === -1 ? text : text.slice(0, slash)).trim();
  const columns = slash === -1 ? "none" : text.slice(slash + 1).trim() || "none";
  if (rowsText.toLowerCase() === "none") return names.map((name) => [name, "none"]);
  const tokens = components(rowsText, /\s/);
  const areas = [];
  const rows = [];
  for (const token of tokens) {
    if (/^["']/.test(token)) {
      // A browser quotes an area row with double quotes, whatever it was written
      // with, and a row with no size after it is `auto`.
      areas.push(`"${token.slice(1, -1)}"`);
      rows.push("auto");
    } else if (areas.length > 0) rows[rows.length - 1] = token;
    else rows.push(token);
  }
  return [
    [names[0], rows.join(" ") || "none"],
    [names[1], columns],
    [names[2], areas.length > 0 ? areas.join(" ") : "none"],
  ];
}

const MASK_REPEATS = new Set(["repeat", "repeat-x", "repeat-y", "no-repeat", "space", "round"]);
const MASK_COMPOSITES = new Set(["add", "subtract", "intersect", "exclude"]);
const MASK_MODES = new Set(["alpha", "luminance", "match-source"]);

// `mask`, read the way `background` is — by what each token can be — with the
// part after a slash as the size. A single position component is doubled, as a
// browser doubles it.
function mask(parts) {
  const found = {
    "mask-image": "none", "mask-position": "0% 0%", "mask-size": "auto", "mask-repeat": "repeat",
    "mask-origin": "border-box", "mask-clip": "border-box", "mask-composite": "add",
    "mask-mode": "match-source",
  };
  const position = [];
  const size = [];
  const boxes = [];
  let afterSlash = false;
  const isSize = (token) => ["auto", "cover", "contain"].includes(token.toLowerCase()) || isLength(token);
  for (const token of parts) {
    if (token === "/") { afterSlash = true; continue; }
    const lower = token.toLowerCase();
    // Only while the tokens still read as a size: `center / cover no-repeat`
    // ends the size at `cover`.
    if (afterSlash && isSize(token)) { size.push(token); continue; }
    afterSlash = false;
    if (isImage(lower) || lower === "none") found["mask-image"] = token;
    else if (MASK_REPEATS.has(lower)) found["mask-repeat"] = token;
    else if (MASK_COMPOSITES.has(lower)) found["mask-composite"] = token;
    else if (MASK_MODES.has(lower)) found["mask-mode"] = token;
    else if (BOXES.has(lower) || lower === "fill-box" || lower === "stroke-box" || lower === "view-box") boxes.push(token);
    else if (POSITIONS.has(lower) || isLength(token)) position.push(token);
  }
  if (position.length === 1) found["mask-position"] = `${position[0]} center`;
  else if (position.length > 1) found["mask-position"] = position.join(" ");
  if (size.length) found["mask-size"] = size.join(" ");
  if (boxes.length) {
    found["mask-origin"] = boxes[0];
    found["mask-clip"] = boxes[1] ?? boxes[0];
  }
  // In the order a browser enumerates them, which is the family's grammar order
  // rather than the order this code fills them in.
  return [
    "mask-image", "mask-position", "mask-size", "mask-repeat", "mask-origin", "mask-clip",
    "mask-composite", "mask-mode",
  ].map((name) => [name, found[name]]);
}

// `offset`: a path, how far along it, which way round, and — after a slash — the
// anchor.
function offset(parts, text) {
  const found = {
    "offset-position": "normal", "offset-path": "none", "offset-distance": "0",
    "offset-rotate": "auto", "offset-anchor": "auto",
  };
  const slash = text.indexOf("/");
  const head = components((slash === -1 ? text : text.slice(0, slash)).trim(), /\s/);
  if (slash !== -1) found["offset-anchor"] = text.slice(slash + 1).trim() || "auto";
  const rotate = [];
  for (const token of head) {
    const lower = token.toLowerCase();
    if (/^(path|ray|url|circle|ellipse|inset|polygon|rect|xywh)\(/i.test(lower) || lower === "none") found["offset-path"] = token;
    else if (lower === "auto" || lower === "reverse" || /^[+-]?[\d.]+(deg|grad|rad|turn)$/i.test(lower)) rotate.push(token);
    else if (isLength(token)) found["offset-distance"] = token;
    else found["offset-position"] = token;
  }
  if (rotate.length) found["offset-rotate"] = rotate.join(" ");
  return Object.entries(found);
}

const SHORTHANDS = new Map(Object.entries({
  margin: (v) => SIDES.map((side, i) => [`margin-${side}`, edges(v)[i]]),
  padding: (v) => SIDES.map((side, i) => [`padding-${side}`, edges(v)[i]]),
  inset: (v) => SIDES.map((side, i) => [side, edges(v)[i]]),
  "scroll-margin": (v) => SIDES.map((side, i) => [`scroll-margin-${side}`, edges(v)[i]]),
  "scroll-padding": (v) => SIDES.map((side, i) => [`scroll-padding-${side}`, edges(v)[i]]),
  "border-width": (v) => SIDES.map((side, i) => [`border-${side}-width`, edges(v)[i]]),
  "border-style": (v) => SIDES.map((side, i) => [`border-${side}-style`, edges(v)[i]]),
  "border-color": (v) => SIDES.map((side, i) => [`border-${side}-color`, edges(v)[i]]),
  "border-radius": radii,
  border: (v) => {
    const { width, style, color } = lineParts(v);
    return [
      ...SIDES.map((side) => [`border-${side}-width`, width ?? "medium"]),
      ...SIDES.map((side) => [`border-${side}-style`, style ?? "none"]),
      ...SIDES.map((side) => [`border-${side}-color`, color ?? "currentcolor"]),
      // The shorthand resets the border image, which is why a declaration that
      // can no longer be written as `border` still says `border-image: initial`.
      ...BORDER_IMAGE.map((name) => [name, "initial"]),
    ];
  },
  "border-top": (v) => line("border-top", v),
  "border-right": (v) => line("border-right", v),
  "border-bottom": (v) => line("border-bottom", v),
  "border-left": (v) => line("border-left", v),
  outline: (v) => line("outline", v),
  "margin-block": (v) => [["margin-block-start", v[0]], ["margin-block-end", v[1] ?? v[0]]],
  "margin-inline": (v) => [["margin-inline-start", v[0]], ["margin-inline-end", v[1] ?? v[0]]],
  "padding-block": (v) => [["padding-block-start", v[0]], ["padding-block-end", v[1] ?? v[0]]],
  "padding-inline": (v) => [["padding-inline-start", v[0]], ["padding-inline-end", v[1] ?? v[0]]],
  "inset-block": (v) => [["inset-block-start", v[0]], ["inset-block-end", v[1] ?? v[0]]],
  "inset-inline": (v) => [["inset-inline-start", v[0]], ["inset-inline-end", v[1] ?? v[0]]],
  "border-image": (v, text) => {
    const groups = text.split("/").map((group) => components(group.trim(), /\s/));
    const found = {
      "border-image-source": "none", "border-image-slice": "100%",
      "border-image-width": "1", "border-image-outset": "0", "border-image-repeat": "stretch",
    };
    const repeats = new Set(["stretch", "repeat", "round", "space"]);
    const slice = [];
    for (const token of groups[0] ?? []) {
      const lower = token.toLowerCase();
      if (isImage(lower) || lower === "none") found["border-image-source"] = token;
      else if (repeats.has(lower)) found["border-image-repeat"] = token;
      else slice.push(token);
    }
    if (slice.length) found["border-image-slice"] = slice.join(" ");
    if (groups[1]?.length) found["border-image-width"] = groups[1].join(" ");
    if (groups[2]?.length) found["border-image-outset"] = groups[2].join(" ");
    return Object.entries(found);
  },
  "background-position": (v) => [
    ["background-position-x", v[0]],
    ["background-position-y", v[1] ?? "center"],
  ],
  gap: (v) => [["row-gap", v[0]], ["column-gap", v[1] ?? v[0]]],
  overflow: (v) => [["overflow-x", v[0]], ["overflow-y", v[1] ?? v[0]]],
  "overscroll-behavior": (v) => [["overscroll-behavior-x", v[0]], ["overscroll-behavior-y", v[1] ?? v[0]]],
  "place-content": (v) => [["align-content", v[0]], ["justify-content", v[1] ?? v[0]]],
  "place-items": (v) => [["align-items", v[0]], ["justify-items", v[1] ?? v[0]]],
  "place-self": (v) => [["align-self", v[0]], ["justify-self", v[1] ?? v[0]]],
  "flex-flow": (v) => {
    const wraps = new Set(["wrap", "nowrap", "wrap-reverse"]);
    const wrap = v.find((token) => wraps.has(token.toLowerCase()));
    const direction = v.find((token) => !wraps.has(token.toLowerCase()));
    return [["flex-direction", direction ?? "row"], ["flex-wrap", wrap ?? "nowrap"]];
  },
  flex,
  font,
  background,
  columns: (v) => {
    const count = v.find((token) => NUMBER.test(token));
    const width = v.find((token) => token !== count);
    return [["column-width", width ?? "auto"], ["column-count", count ?? "auto"]];
  },
  "list-style": (v) => {
    const found = { "list-style-position": "outside", "list-style-image": "none", "list-style-type": "disc" };
    for (const token of v) {
      const lower = token.toLowerCase();
      if (LIST_POSITIONS.has(lower)) found["list-style-position"] = token;
      else if (isImage(lower)) found["list-style-image"] = token;
      else found["list-style-type"] = token;
    }
    return Object.entries(found);
  },
  "text-decoration": (v) => {
    const lines = new Set(["none", "underline", "overline", "line-through", "blink"]);
    const styles = new Set(["solid", "double", "dotted", "dashed", "wavy"]);
    const found = { "text-decoration-line": "none", "text-decoration-style": "solid", "text-decoration-color": "currentcolor", "text-decoration-thickness": "auto" };
    const written = [];
    for (const token of v) {
      const lower = token.toLowerCase();
      if (lines.has(lower)) written.push(token);
      else if (styles.has(lower)) found["text-decoration-style"] = token;
      else if (lower === "auto" || lower === "from-font" || isLength(token)) found["text-decoration-thickness"] = token;
      else found["text-decoration-color"] = token;
    }
    if (written.length) found["text-decoration-line"] = written.join(" ");
    return Object.entries(found);
  },
  "grid-row": (v) => ends(["grid-row-start", "grid-row-end"], v, "auto"),
  "grid-column": (v) => ends(["grid-column-start", "grid-column-end"], v, "auto"),
  transition,
  animation,
  "grid-template": gridTemplate,
  mask,
  offset,
  "grid-area": (v) => {
    const parts = v.join(" ").split("/").map((part) => part.trim());
    const [rowStart = "auto", columnStart = "auto", rowEnd = rowStart, columnEnd = columnStart] = parts;
    return [
      ["grid-row-start", rowStart], ["grid-column-start", columnStart],
      ["grid-row-end", rowEnd], ["grid-column-end", columnEnd],
    ];
  },
}));

export const isShorthand = (name) => SHORTHANDS.has(name);

// A value the family's own grammar would accept, so the longhand *names* can be
// had without a value to expand — which is what a shorthand written with
// `var()` needs, since its parts are not known until the custom property is
// substituted at computed-value time.
const SAMPLES = {
  "border-image": "url(b.png) 30 / 10px / 2px round",
  transition: "opacity 2s",
  animation: "spin 2s",
  "grid-template": "1fr / 1fr",
  mask: "url(m.svg)",
  offset: "path('M 0 0') 0%",
  font: "italic small-caps bold 10px/1 serif",
  background: "url(x) no-repeat red",
  flex: "1 1 auto",
  "flex-flow": "row wrap",
  columns: "10px 2",
  "list-style": "disc outside none",
  "text-decoration": "underline solid red",
  "grid-row": "1 / 2",
  "grid-column": "1 / 2",
  "grid-area": "1 / 2 / 3 / 4",
};

export function shorthandLonghands(name) {
  const property = String(name).toLowerCase();
  if (!SHORTHANDS.has(property)) return [];
  return expandShorthand(property, SAMPLES[property] ?? "1px 1px 1px 1px").map(([longhand]) => longhand);
}

/// The longhands a shorthand declaration sets, or an empty list for a property
/// that is not one of the families above.
export function expandShorthand(name, value) {
  const expand = SHORTHANDS.get(String(name).toLowerCase());
  if (!expand) return [];
  const text = String(value).trim();
  // Whitespace only: a comma belongs to the value it follows, so a font family
  // list or a layered background keeps its shape.
  const parts = components(text, /\s/);
  if (parts.length === 0) return [];
  // A global keyword sets every longhand of the shorthand to itself, whatever
  // the family's own grammar is.
  if (parts.length === 1 && GLOBALS.has(parts[0].toLowerCase())) {
    return expand(["0"], "0").map(([longhand]) => [longhand, parts[0]]);
  }
  // A value with `var()` in it cannot be split before substitution, and this
  // runs before that: the shorthand stays whole rather than being guessed at.
  if (text.includes("var(")) return [];
  return expand(parts, text).filter(([, longhand]) => longhand !== undefined && longhand !== null);
}
