// Stylesheets and the cascade for esdev's test DOM.
//
// It resolves *specified* values: origin, importance, specificity and order,
// then inheritance, then the initial value. Nothing here resolves a used value,
// because that needs layout — a percentage stays a percentage, `auto` stays
// `auto`, and a length is whatever was written. What it does answer correctly is
// which declaration won, which is the question a component test asks.

const SHEETS = Symbol("esdev DOM style sheets");
const ADOPTED = Symbol("esdev DOM adopted style sheets");
const RULES = Symbol("esdev DOM css rules");
const OWNER = Symbol("esdev DOM sheet owner");

const STYLE_RULE = 0;
const GROUP_RULE = 1;
const OTHER_RULE = 2;

// The properties a child takes from its parent when nothing else sets them.
// Trimmed to what a layout-free DOM can be asked about, which is text and
// inheritance-driven state rather than box geometry.
const INHERITED = new Set([
  "azimuth", "border-collapse", "border-spacing", "caption-side", "caret-color", "color",
  "color-scheme", "cursor", "direction", "empty-cells", "font", "font-family", "font-feature-settings",
  "font-kerning", "font-optical-sizing", "font-size", "font-size-adjust", "font-stretch", "font-style",
  "font-variant", "font-variant-caps", "font-variant-ligatures", "font-variant-numeric", "font-weight",
  "forced-color-adjust", "hyphens", "image-rendering", "letter-spacing", "line-break", "line-height",
  "list-style", "list-style-image", "list-style-position", "list-style-type", "orphans", "overflow-wrap",
  "paint-order", "pointer-events", "quotes", "tab-size", "text-align", "text-align-last", "text-anchor",
  "text-combine-upright", "text-decoration-color", "text-indent", "text-justify", "text-orientation",
  "text-rendering", "text-shadow", "text-size-adjust", "text-transform", "text-underline-offset",
  "text-underline-position", "text-wrap", "text-wrap-mode", "text-wrap-style", "visibility",
  "white-space", "white-space-collapse", "widows", "word-break", "word-spacing", "writing-mode",
]);

// The user-agent stylesheet, reduced to what a layout-free DOM can honestly
// report: which elements are blocks, which are hidden, and the handful of
// text defaults a test asserts on. A browser's is thousands of declarations
// long, and the rest of them describe how something looks.
const UA_RULES = [
  ["html, body, div, p, h1, h2, h3, h4, h5, h6, ol, ul, li, dl, dt, dd, figure, figcaption, main, header, footer, section, article, aside, nav, address, blockquote, pre, listing, xmp, plaintext, hr, form, fieldset, legend, details, summary, dialog, search, hgroup, menu", { display: "block" }],
  ["li", { display: "list-item" }],
  ["table", { display: "table" }],
  ["thead", { display: "table-header-group" }],
  ["tbody", { display: "table-row-group" }],
  ["tfoot", { display: "table-footer-group" }],
  ["tr", { display: "table-row" }],
  ["td, th", { display: "table-cell" }],
  ["caption", { display: "table-caption" }],
  ["colgroup", { display: "table-column-group" }],
  ["col", { display: "table-column" }],
  ["head, link, meta, style, script, noscript, title, template, base, param, source, track, area", { display: "none" }],
  ["[hidden]", { display: "none" }],
  ["b, strong", { "font-weight": "700" }],
  ["i, em, cite, var, dfn, address", { "font-style": "italic" }],
  ["h1", { "font-size": "2em", "font-weight": "700" }],
  ["h2", { "font-size": "1.5em", "font-weight": "700" }],
  ["h3", { "font-size": "1.17em", "font-weight": "700" }],
  ["h4", { "font-weight": "700" }],
  ["h5", { "font-size": "0.83em", "font-weight": "700" }],
  ["h6", { "font-size": "0.67em", "font-weight": "700" }],
  ["pre, code, kbd, samp, tt", { "font-family": "monospace" }],
  ["pre, listing, xmp, plaintext", { "white-space": "pre" }],
  ["textarea", { "white-space": "pre-wrap" }],
  ["nobr", { "white-space": "nowrap" }],
  ["center, caption, th", { "text-align": "center" }],
  ["ul, ol", { "list-style-type": "disc" }],
  ["ol", { "list-style-type": "decimal" }],
];

// What each display computes to when it is blockified; one absent from the
// table is already block-level, or `contents`/`none`, and stays as it is.
const BLOCKIFY = new Map([
  ["inline", "block"], ["inline-block", "block"], ["inline-flex", "flex"], ["inline-grid", "grid"],
  ["inline-table", "table"], ["ruby", "block ruby"],
  ...["table-row", "table-cell", "table-row-group", "table-header-group", "table-footer-group",
    "table-column", "table-column-group", "table-caption"].map((display) => [display, "block"]),
]);

// Initial values, for the properties whose initial value is a keyword a test can
// assert on. A length or a colour that a browser resolves against layout or a
// colour scheme is deliberately absent: answering `""` is honest, answering a
// number this DOM did not compute would not be.
const INITIAL = new Map(Object.entries({
  "display": "inline",
  "visibility": "visible",
  "direction": "ltr",
  "font-style": "normal",
  "font-weight": "400",
  "font-variant": "normal",
  "text-align": "start",
  "text-transform": "none",
  "white-space": "normal",
  "list-style-position": "outside",
  "list-style-type": "disc",
  "border-collapse": "separate",
  "pointer-events": "auto",
  "position": "static",
  "color": "rgb(0, 0, 0)",
  // Keyword and zero initials, each one checked against Chrome: these do not
  // depend on layout, so answering them is not a guess. The sizing family
  // (`width`, `height`, `inline-size`, …) is deliberately absent, because a
  // browser answers those with a used value in pixels.
  "transform": "none",
  "opacity": "1",
  "overflow-x": "visible",
  "overflow-y": "visible",
  "float": "none",
  "clear": "none",
  "box-sizing": "content-box",
  "flex-direction": "row",
  "flex-wrap": "nowrap",
  "flex-grow": "0",
  "flex-shrink": "1",
  "flex-basis": "auto",
  "align-items": "normal",
  "justify-content": "normal",
  "order": "0",
  "row-gap": "normal",
  "column-gap": "normal",
  "grid-template-columns": "none",
  "grid-template-rows": "none",
  "background-color": "rgba(0, 0, 0, 0)",
  "background-image": "none",
  "border-top-style": "none",
  "border-right-style": "none",
  "border-bottom-style": "none",
  "border-left-style": "none",
  "border-top-width": "0px",
  "border-right-width": "0px",
  "border-bottom-width": "0px",
  "border-left-width": "0px",
  "border-top-left-radius": "0px",
  "border-top-right-radius": "0px",
  "border-bottom-right-radius": "0px",
  "border-bottom-left-radius": "0px",
  "outline-style": "none",
  "text-decoration-line": "none",
  "text-overflow": "clip",
  "word-break": "normal",
  "line-height": "normal",
  "cursor": "auto",
  "z-index": "auto",
  "vertical-align": "baseline",
  "user-select": "auto",
  "mix-blend-mode": "normal",
  "isolation": "auto",
  "object-fit": "fill",
  "resize": "none",
  "appearance": "none",
  "table-layout": "auto",
  "aspect-ratio": "auto",
  "will-change": "auto",
  "filter": "none",
  "animation-name": "none",
  "top": "auto",
  "right": "auto",
  "bottom": "auto",
  "left": "auto",
  "margin-top": "0px",
  "margin-right": "0px",
  "margin-bottom": "0px",
  "margin-left": "0px",
  "padding-top": "0px",
  "padding-right": "0px",
  "padding-bottom": "0px",
  "padding-left": "0px",
}));

// ---------------------------------------------------------------------------
// Absolute lengths
//
// A computed style reports lengths in pixels, and most of that conversion needs
// no layout: an absolute unit is a fixed multiple of a pixel, `em` is a multiple
// of the element's own font size, and `rem` of the root's. So those are resolved
// here, and the ones that genuinely need a box — a percentage of the containing
// block, `ex` and `ch` (font metrics), the viewport and container units — are
// left exactly as written, which is this DOM's standing answer for a value it
// cannot compute.
// ---------------------------------------------------------------------------

const ROOT_FONT_SIZE = 16;

// Each unit as a multiple of a pixel, at the 96dpi CSS reference.
const ABSOLUTE_UNITS = {
  px: 1, pt: 96 / 72, pc: 16, in: 96, cm: 96 / 2.54, mm: 96 / 25.4, q: 96 / 101.6,
};

// The absolute keyword sizes, and the two relative ones, checked against Chrome.
const FONT_SIZE_KEYWORDS = {
  "xx-small": 9, "x-small": 10, small: 13, medium: 16, large: 18,
  "x-large": 24, "xx-large": 32, "xxx-large": 48,
};

const LENGTH = /^([+-]?(?:\d+\.?\d*|\.\d+)(?:e[+-]?\d+)?)([a-z%]*)$/i;

// As a browser prints one: four decimals at most, and no trailing zeros.
function printPx(value) {
  return `${Number(value.toFixed(4))}px`;
}

// One component as pixels, or null when it is not a length this DOM can make
// absolute.
function toPixels(component, fontSize, rootFontSize) {
  const match = LENGTH.exec(component);
  if (match === null) return null;
  const number = Number(match[1]);
  const unit = match[2].toLowerCase();
  if (unit === "" ) return number === 0 ? 0 : null;
  if (unit === "em") return number * fontSize;
  if (unit === "rem") return number * rootFontSize;
  const factor = ABSOLUTE_UNITS[unit];
  return factor === undefined ? null : number * factor;
}

// The element's own font size in pixels, from whatever was declared for it.
function resolveFontSize(declared, parentSize, rootFontSize) {
  if (declared === undefined) return parentSize;
  const value = String(declared).trim().toLowerCase();
  if (Object.hasOwn(FONT_SIZE_KEYWORDS, value)) return FONT_SIZE_KEYWORDS[value];
  if (value === "smaller") return parentSize / 1.2;
  if (value === "larger") return parentSize * 1.2;
  const percent = /^([+-]?[\d.]+)%$/.exec(value);
  if (percent) return (Number(percent[1]) / 100) * parentSize;
  // `em` on `font-size` is a multiple of the *parent's* size, not its own.
  const pixels = toPixels(value, parentSize, rootFontSize);
  return pixels === null ? parentSize : pixels;
}

// `line-height`, which takes a number as a multiple of the font size and a
// percentage as one too. `normal` stays `normal`: what it resolves to is the
// font's own metric.
function resolveLineHeight(declared, fontSize, rootFontSize) {
  if (declared === undefined) return undefined;
  const value = String(declared).trim().toLowerCase();
  if (value === "normal" || value === "") return declared;
  const number = /^[+-]?(?:\d+\.?\d*|\.\d+)$/.exec(value);
  if (number) return printPx(Number(value) * fontSize);
  const percent = /^([+-]?[\d.]+)%$/.exec(value);
  if (percent) return printPx((Number(percent[1]) / 100) * fontSize);
  const pixels = toPixels(value, fontSize, rootFontSize);
  return pixels === null ? declared : printPx(pixels);
}

// Where an element's own `style` attribute sits, above every author rule that
// is not `!important`.
const ORIGIN = { ua: 0, author: 1, inline: 2 };

// A media query list as Chrome writes it back: lowercased, one space after a
// feature's colon, queries joined by ", " — and a query that is empty or does
// not start like one reads "not all".
const MEDIA_LIST = Symbol("esdev CSS media list");
function mediaQueries(text) {
  text = String(text);
  if (text.trim() === "") return [];
  const queries = [];
  let depth = 0;
  let current = "";
  for (const character of text) {
    if (character === "(") depth += 1;
    else if (character === ")") depth = Math.max(0, depth - 1);
    if (character === "," && depth === 0) {
      queries.push(current);
      current = "";
    } else current += character;
  }
  queries.push(current);
  return queries.map((query) => {
    const normal = query.trim().replace(/\s+/g, " ").replace(/[A-Z]+/g, (letters) => letters.toLowerCase())
      .replace(/\(\s*/g, "(").replace(/\s*\)/g, ")").replace(/\(([a-z-]+)\s*:\s*/g, "($1: ");
    return /^[a-z(]/.test(normal) ? normal : "not all";
  });
}
class MediaList {
  constructor(text) {
    Object.defineProperty(this, MEDIA_LIST, { value: { queries: mediaQueries(text) } });
    return new Proxy(this, {
      get(target, property, receiver) {
        if (typeof property === "string" && /^(0|[1-9][0-9]*)$/.test(property)) return target[MEDIA_LIST].queries[Number(property)];
        return Reflect.get(target, property, receiver);
      },
    });
  }
  get mediaText() { return this[MEDIA_LIST].queries.join(", "); }
  set mediaText(value) { this[MEDIA_LIST].queries = mediaQueries(value ?? ""); }
  get length() { return this[MEDIA_LIST].queries.length; }
  item(index) { return this[MEDIA_LIST].queries[Number(index)] ?? null; }
  appendMedium(medium) {
    const [query] = mediaQueries(medium);
    if (query !== undefined && !this[MEDIA_LIST].queries.includes(query)) this[MEDIA_LIST].queries.push(query);
  }
  deleteMedium(medium) {
    const [query] = mediaQueries(medium);
    const list = this[MEDIA_LIST].queries;
    if (!list.includes(query)) throw new DOMException("The medium is not in the list.", "NotFoundError");
    this[MEDIA_LIST].queries = list.filter((each) => each !== query);
  }
  toString() { return this.mediaText; }
}
MediaList.prototype[Symbol.iterator] = Array.prototype.values;

export function createSheets({ tree, parse, selectors, css, mediaMatches, colors = null }) {
  const { Document, Element, ShadowRoot, HTML_NAMESPACE } = tree;
  const types = css.types ?? null;

  class CSSRuleList {
    constructor(rules) {
      Object.defineProperty(this, RULES, { value: rules });
      for (const [index, rule] of rules.entries()) Object.defineProperty(this, index, { value: rule, enumerable: true });
    }
    get length() { return this[RULES].length; }
    item(index) { return this[RULES][Number(index)] ?? null; }
  }

  class StyleSheetList {
    constructor(sheets) {
      Object.defineProperty(this, RULES, { value: sheets });
      for (const [index, sheet] of sheets.entries()) Object.defineProperty(this, index, { value: sheet, enumerable: true });
    }
    get length() { return this[RULES].length; }
    item(index) { return this[RULES][Number(index)] ?? null; }
  }

  CSSRuleList.prototype[Symbol.iterator] = Array.prototype.values;
  StyleSheetList.prototype[Symbol.iterator] = Array.prototype.values;

  class CSSRule {
    get cssText() { return ""; }
  }

  class CSSStyleRule extends CSSRule {
    constructor(selectorText, declarations) {
      super();
      this.selectorText = selectorText;
      // Parsed once: what this selector means depends on the root the rule
      // came from, and that question is asked for every element.
      Object.defineProperty(this, "shadow", { value: shadowSelector(selectorText) });
      // Read-only: a rule's declarations are not a place this DOM lets a test
      // write, because nothing would reparse the sheet afterwards.
      this.style = css.readOnlyDeclaration(declarations);
      Object.defineProperty(this, RULES, { value: declarations });
    }
    get cssText() {
      const body = this[RULES]
        .map(([name, value, important]) => `${name}: ${value}${important ? " !important" : ""};`)
        .join(" ");
      return `${this.selectorText} { ${body} }`;
    }
  }

  // A grouping rule's at-keyword and prelude are its own business: `name` is
  // a layer's name on CSSLayerBlockRule, not "media".
  const AT_RULE = Symbol("esdev CSS at-rule");
  class CSSGroupingRule extends CSSRule {
    constructor(keyword, prelude, rules) {
      super();
      Object.defineProperty(this, AT_RULE, { value: { keyword, prelude } });
      this.cssRules = new CSSRuleList(rules);
    }
    // Chrome's serialization: the prelude, then each rule on its own indented
    // line.
    get cssText() {
      const { keyword, prelude } = this[AT_RULE];
      const rules = Array.from(this.cssRules, (rule) => `\n  ${rule.cssText}`).join("");
      return `@${keyword}${prelude ? ` ${prelude}` : ""} {${rules}\n}`;
    }
  }
  class CSSConditionRule extends CSSGroupingRule {
    get conditionText() { return this[AT_RULE].prelude; }
  }
  class CSSMediaRule extends CSSConditionRule {
    constructor(keyword, prelude, rules) {
      const media = new MediaList(prelude);
      super(keyword, media.mediaText, rules);
      Object.defineProperty(this, MEDIA_LIST, { value: media });
    }
    // [SameObject, PutForwards=mediaText].
    get media() { return this[MEDIA_LIST]; }
    set media(value) { this[MEDIA_LIST].mediaText = value; }
    get conditionText() { return this[MEDIA_LIST].mediaText; }
    get cssText() {
      const rules = Array.from(this.cssRules, (rule) => `\n  ${rule.cssText}`).join("");
      return `@media ${this.conditionText} {${rules}\n}`;
    }
  }
  class CSSSupportsRule extends CSSConditionRule {}
  class CSSContainerRule extends CSSConditionRule {}
  class CSSLayerBlockRule extends CSSGroupingRule {
    get name() { return this[AT_RULE].prelude; }
  }
  const GROUPING_RULES = { media: CSSMediaRule, supports: CSSSupportsRule, container: CSSContainerRule, layer: CSSLayerBlockRule };

  class CSSOtherRule extends CSSRule {
    constructor(name, prelude) {
      super();
      Object.defineProperty(this, "name", { value: name });
      this.conditionText = prelude;
    }
    get cssText() { return `@${this.name} ${this.conditionText}`; }
  }

  function ruleFrom(record) {
    const [kind, first, second, children] = record;
    // A declaration whose value the DOM cannot parse is dropped from the rule,
    // the way a browser drops it: the rest of the block still applies.
    if (kind === STYLE_RULE) return new CSSStyleRule(first, second.filter(([name, value]) => css.keepsDeclaration(name, value)));
    if (kind === GROUP_RULE) return new (GROUPING_RULES[first] ?? CSSGroupingRule)(first, second, children.map(ruleFrom));
    if (kind === OTHER_RULE) return new CSSOtherRule(first, second);
    throw new TypeError(`Unsupported CSS rule kind: ${kind}`);
  }

  class CSSStyleSheet {
    constructor(options = {}) {
      Object.defineProperty(this, RULES, { value: { rules: [], version: 0 } });
      Object.defineProperty(this, MEDIA_LIST, { value: new MediaList(options.media instanceof MediaList ? options.media.mediaText : String(options.media ?? "")) });
      this.disabled = false;
      this.cssRules = new CSSRuleList([]);
      Object.defineProperty(this, OWNER, { value: { node: null }, writable: true });
    }
    get ownerNode() { return this[OWNER].node; }
    // [SameObject, PutForwards=mediaText].
    get media() { return this[MEDIA_LIST]; }
    set media(value) { this[MEDIA_LIST].mediaText = value; }
    get type() { return "text/css"; }
    replaceSync(text) {
      const rules = parse(String(text)).map(ruleFrom);
      this[RULES].rules = rules;
      this[RULES].version += 1;
      this.cssRules = new CSSRuleList(rules);
    }
    replace(text) {
      // Nothing here fetches, so the async form differs only in when it resolves.
      try {
        this.replaceSync(text);
        return Promise.resolve(this);
      } catch (error) {
        return Promise.reject(error);
      }
    }
    insertRule(text, index = 0) {
      const rules = parse(String(text)).map(ruleFrom);
      if (rules.length !== 1) throw new DOMException("insertRule takes exactly one rule.", "SyntaxError");
      const list = this[RULES].rules;
      if (index > list.length) throw new DOMException("The index is past the end of the rule list.", "IndexSizeError");
      list.splice(Number(index), 0, rules[0]);
      this[RULES].version += 1;
      this.cssRules = new CSSRuleList(list);
      return Number(index);
    }
    deleteRule(index) {
      const list = this[RULES].rules;
      if (index >= list.length) throw new DOMException("The index is past the end of the rule list.", "IndexSizeError");
      list.splice(Number(index), 1);
      this[RULES].version += 1;
      this.cssRules = new CSSRuleList(list);
    }
  }

  function sheetRules(sheet) {
    return sheet.disabled ? [] : sheet[RULES].rules;
  }

  // A `<style>` element's sheet, kept in step with its text. The element owns
  // it, so a test can read `style.sheet.cssRules` the way it would in a browser.
  function styleSheetFor(element) {
    let state = element[SHEETS];
    if (!state) {
      state = { text: null, sheet: new CSSStyleSheet() };
      state.sheet[OWNER].node = element;
      Object.defineProperty(element, SHEETS, { value: state });
    }
    const media = element.getAttribute("media") ?? "";
    if (state.sheet.media.mediaText !== mediaQueries(media).join(", ")) state.sheet.media.mediaText = media;
    const text = element.textContent ?? "";
    if (state.text !== text) {
      state.sheet.replaceSync(text);
      state.text = text;
    }
    return state.sheet;
  }

  function documentSheets(root) {
    const document = root instanceof Document ? root : root.ownerDocument;
    const scope = root instanceof ShadowRoot ? root : document;
    const found = [];
    const walk = (node) => {
      for (let child = node.firstChild; child; child = child.nextSibling) {
        if (!(child instanceof Element)) continue;
        if (child.localName === "style" && child.namespaceURI === HTML_NAMESPACE) found.push(styleSheetFor(child));
        walk(child);
      }
    };
    walk(scope);
    return [...found, ...(scope[ADOPTED] ?? [])];
  }

  // --- the cascade ---------------------------------------------------------

  function conditionHolds(rule) {
    if (rule instanceof CSSMediaRule) return mediaMatches(rule.conditionText);
    if (rule instanceof CSSSupportsRule) return supportsCondition(rule.conditionText);
    // `@layer`, `@scope` and `@container` blocks always contribute here: layer
    // ordering is not implemented, and a container query has no container.
    return true;
  }

  // `@supports` is answered by what this DOM can parse, not by what it can
  // render: a declaration it keeps is supported, a selector it matches is too.
  function supportsCondition(text) {
    const condition = String(text).trim();
    if (/^not\s+/i.test(condition)) return !supportsCondition(condition.replace(/^not\s+/i, ""));
    if (/\s+and\s+/i.test(condition)) return condition.split(/\s+and\s+/i).every(supportsCondition);
    if (/\s+or\s+/i.test(condition)) return condition.split(/\s+or\s+/i).some(supportsCondition);
    const unwrapped = /^\((.*)\)$/s.exec(condition);
    const inner = (unwrapped ? unwrapped[1] : condition).trim();
    const selector = /^selector\((.*)\)$/is.exec(inner);
    if (selector) {
      try {
        selectors.compile(selector[1]);
        return true;
      } catch {
        return false;
      }
    }
    const colon = inner.indexOf(":");
    if (colon <= 0) return false;
    const name = inner.slice(0, colon).trim();
    const value = inner.slice(colon + 1).trim();
    return css.supportsDeclaration(name, value);
  }

  // Rules that could apply to an element, flattened out of their groups, with
  // the specificity and order the cascade sorts by.
  // The counter is shared across every sheet an element sees, not restarted per
  // sheet: order is what breaks a tie between two rules of equal specificity, and
  // a per-sheet counter gave the first sheet's rule the same order as the
  // second's — so which won depended on the order they happened to be examined
  // in. A document's `<style>` elements come before its adopted sheets, which is
  // why Lit's static styles beat the markup its render writes.
  function* applicable(rules, origin, counter) {
    for (const rule of rules) {
      if (rule instanceof CSSGroupingRule) {
        if (conditionHolds(rule)) yield* applicable(Array.from(rule.cssRules), origin, counter);
        continue;
      }
      if (!(rule instanceof CSSStyleRule)) continue;
      counter.next += 1;
      yield { rule, origin, order: counter.next };
    }
  }

  // The selectors that cross a shadow boundary. They are read here rather than
  // in the selector engine because what they mean depends on *where the rule
  // came from*: `:host` is the root's host, and `::slotted()` is a light-DOM
  // node this root's slots took in. A selector API has no such root, and
  // `element.matches(":host")` is meaningless outside one.
  const SHADOW_SELECTOR = /^\s*(:host-context\(|:host\(|:host\b|(.*?)::slotted\()/;

  function shadowSelector(text) {
    const source = String(text);
    if (!SHADOW_SELECTOR.test(source)) return null;
    if (source.includes("::slotted(")) {
      const at = source.indexOf("::slotted(");
      const inner = balanced(source, at + "::slotted(".length);
      if (inner === null) return null;
      return {
        kind: "slotted",
        slot: source.slice(0, at).trim(),
        inner: inner.text,
        rest: source.slice(inner.end + 1).trim(),
      };
    }
    const context = source.trimStart().startsWith(":host-context(");
    const opens = source.indexOf("(");
    if (!context && !/^\s*:host(\s|$|\()/.test(source)) return null;
    if (opens === -1 || !/^\s*:host(-context)?\(/.test(source)) {
      return { kind: "host", inner: null, rest: source.trim().replace(/^:host/, "").trim() };
    }
    const inner = balanced(source, opens + 1);
    if (inner === null) return null;
    return {
      kind: context ? "host-context" : "host",
      inner: inner.text,
      rest: source.slice(inner.end + 1).trim(),
    };
  }

  // The text inside a parenthesis that opens at `from`, and the index of its
  // closing one.
  function balanced(source, from) {
    let depth = 1;
    for (let at = from; at < source.length; at += 1) {
      if (source[at] === "(") depth += 1;
      else if (source[at] === ")" && --depth === 0) return { text: source.slice(from, at).trim(), end: at };
    }
    return null;
  }

  function safeMatches(element, selector) {
    if (!selector) return true;
    try {
      return selectors.matches(element, selector);
    } catch {
      return false;
    }
  }

  // `scope` is the shadow root the rule came from, or null for a document rule.
  function matchesRule(element, rule, scope) {
    const shadow = rule.shadow;
    if (shadow) {
      if (!scope) return false;
      if (shadow.kind === "slotted") {
        const slot = element.assignedSlot;
        if (!slot || slot.getRootNode() !== scope) return false;
        if (shadow.slot && !safeMatches(slot, shadow.slot)) return false;
        return safeMatches(element, shadow.inner);
      }
      const host = scope.host;
      if (!host) return false;
      const hostMatches = shadow.kind === "host-context"
        ? ancestorsOf(host).some((candidate) => safeMatches(candidate, shadow.inner))
        : safeMatches(host, shadow.inner);
      if (!hostMatches) return false;
      // `:host(…) .inner` styles something inside the root; plain `:host`
      // styles the host itself.
      if (shadow.rest) return element.getRootNode() === scope && safeMatches(element, shadow.rest);
      return element === host;
    }
    // A pseudo-element rule styles something that is not this element, and a
    // selector this engine cannot match contributes nothing.
    if (rule.selectorText.includes("::")) return false;
    return safeMatches(element, rule.selectorText);
  }

  function ancestorsOf(element) {
    const chain = [];
    for (let current = element; current instanceof Element; current = current.parentElement) chain.push(current);
    return chain;
  }

  // `:host(c)` is a pseudo-class plus what is inside it; `::slotted(c)` is a
  // pseudo-element plus the same.
  function specificityOf(rule) {
    if (!rule.shadow) return selectors.specificity(rule.selectorText);
    const base = rule.shadow.kind === "slotted" ? [0, 0, 1] : [0, 1, 0];
    const inner = rule.shadow.inner ? selectors.specificity(rule.shadow.inner) : [0, 0, 0];
    const rest = rule.shadow.rest ? selectors.specificity(rule.shadow.rest) : [0, 0, 0];
    return [base[0] + inner[0] + rest[0], base[1] + inner[1] + rest[1], base[2] + inner[2] + rest[2]];
  }

  function declared(element) {
    const document = element.ownerDocument;
    const root = element.getRootNode();
    const entries = [];
    // One counter for the whole cascade this element sees.
    const counter = { next: 0 };
    const collect = (rules, origin, scope = null) => {
      for (const { rule, order } of applicable(rules, origin, counter)) {
        if (!matchesRule(element, rule, scope)) continue;
        const specificity = specificityOf(rule);
        for (const [name, value, important] of rule[RULES]) {
          for (const [longhand, part, shorthand] of declarations(property(name), value)) {
            entries.push({ name: longhand, value: part, shorthand, important, origin, specificity, order });
          }
        }
      }
    };
    collect(uaRules, ORIGIN.ua);
    // A shadow root's sheets apply inside it; the document's apply outside.
    const scope = root instanceof ShadowRoot ? root : null;
    for (const sheet of documentSheets(scope ?? document)) {
      collect(sheetRules(sheet), ORIGIN.author, scope);
    }
    // Two sheets from across the boundary: the element's own root styles it
    // through `:host`, and the root a slot took it into styles it through
    // `::slotted()`.
    const hosted = element._esdevShadowRoot?.();
    if (hosted) {
      for (const sheet of documentSheets(hosted)) collect(sheetRules(sheet), ORIGIN.author, hosted);
    }
    const slot = element.assignedSlot;
    const slotRoot = slot?.getRootNode();
    if (slotRoot instanceof ShadowRoot && slotRoot !== scope) {
      for (const sheet of documentSheets(slotRoot)) collect(sheetRules(sheet), ORIGIN.author, slotRoot);
    }
    for (const [name, entry] of element.style ? inlineEntries(element) : []) {
      for (const [longhand, value, shorthand] of declarations(name, entry.value)) {
        entries.push({ name: longhand, value, shorthand, important: entry.priority === "important", origin: ORIGIN.inline, specificity: [0, 0, 0], order: 0 });
      }
    }
    return entries;
  }

  // What one declaration contributes to the cascade. A shorthand contributes
  // its longhands *and* itself: nothing computes from the shorthand, but
  // `getComputedStyle(el).border` should still answer what was written rather
  // than nothing at all.
  function declarations(name, value) {
    const expanded = css.expandShorthand(name, value);
    // Only the longhands: a computed style holds no shorthands, and one asked of
    // it is serialized back out of the parts — which is how `border` reports the
    // colour it computed rather than the name that was written.
    if (expanded.length > 0) return expanded;
    // A shorthand written with `var()` cannot be split until the custom
    // property is substituted, which happens when the value is computed. Its
    // longhands take the whole text now and are expanded then — the
    // specification's "pending substitution value", under a plainer name.
    if (String(value).includes("var(")) {
      const longhands = css.shorthandLonghands(name);
      if (longhands.length > 0) return longhands.map((longhand) => [longhand, value, name]);
    }
    return [[name, value]];
  }

  function inlineEntries(element) {
    const style = element.style;
    const out = [];
    for (let index = 0; index < style.length; index += 1) {
      const name = style.item(index);
      out.push([name, { value: style.getPropertyValue(name), priority: style.getPropertyPriority(name) }]);
    }
    return out;
  }

  function property(name) {
    const text = String(name).trim();
    return text.startsWith("--") ? text : text.toLowerCase();
  }

  // The cascade's origin order, lowest first: normal user-agent, normal author,
  // the style attribute, important author, an important style attribute, and
  // finally important user-agent. A style attribute is an author declaration
  // that outranks every selector, so it is a rank of its own rather than a
  // specificity — an important author rule does not beat an important inline
  // one just by having a class in it.
  function rank(entry) {
    if (!entry.important) return entry.origin === ORIGIN.ua ? 0 : entry.origin === ORIGIN.author ? 2 : 3;
    return entry.origin === ORIGIN.ua ? 7 : entry.origin === ORIGIN.author ? 5 : 6;
  }

  function winner(entries) {
    return entries.reduce((best, entry) => {
      if (!best) return entry;
      if (rank(entry) !== rank(best)) return rank(entry) > rank(best) ? entry : best;
      const bySpecificity = selectors.compareSpecificity(entry.specificity, best.specificity);
      if (bySpecificity !== 0) return bySpecificity > 0 ? entry : best;
      return entry.order >= best.order ? entry : best;
    }, null);
  }

  // `var()` is substituted when a value is computed, from the custom properties
  // in effect on the same element. A name with no value — missing, or caught in
  // a cycle — falls back to the text after the comma; with no fallback the
  // declaration is invalid at computed-value time, which is "unset": the
  // inherited value for an inherited property, the initial value otherwise.
  function substitute(value, values, seen) {
    let out = "";
    let at = 0;
    while (at < value.length) {
      const start = value.indexOf("var(", at);
      if (start === -1) { out += value.slice(at); break; }
      out += value.slice(at, start);
      let depth = 0;
      let end = start + 3;
      for (; end < value.length; end += 1) {
        if (value[end] === "(") depth += 1;
        else if (value[end] === ")") { depth -= 1; if (depth === 0) break; }
      }
      if (depth !== 0) return null;
      const resolved = reference(value.slice(start + 4, end), values, seen);
      if (resolved === null) return null;
      out += resolved;
      at = end + 1;
    }
    return out;
  }

  // The inside of one `var(…)`: a custom property name, then an optional
  // fallback after the first top-level comma.
  function reference(inner, values, seen) {
    let depth = 0;
    let comma = -1;
    for (let at = 0; at < inner.length && comma === -1; at += 1) {
      if (inner[at] === "(") depth += 1;
      else if (inner[at] === ")") depth -= 1;
      else if (inner[at] === "," && depth === 0) comma = at;
    }
    const name = (comma === -1 ? inner : inner.slice(0, comma)).trim();
    const fallback = comma === -1 ? null : inner.slice(comma + 1).trim();
    if (!name.startsWith("--")) return null;
    const declared = seen.has(name) ? undefined : values.get(name);
    const resolved = declared === undefined ? null : substitute(declared, values, new Set(seen).add(name));
    if (resolved !== null && resolved.trim() !== "") return resolved.trim();
    return fallback === null ? null : substitute(fallback, values, seen);
  }

  // Inheritance follows the flat tree, not the node tree: a shadow root's child
  // inherits from the host — which is how a custom property declared on `:host`
  // reaches the markup inside — and a slotted element from the slot it was
  // assigned to rather than from where it was written.
  function flatParent(element) {
    const slot = element.assignedSlot;
    if (slot) return slot;
    const parent = element.parentElement;
    if (parent) return parent;
    const root = element.getRootNode();
    return root instanceof ShadowRoot ? root.host : null;
  }

  // Set only for the length of one read that asks for the computed values of a
  // whole subtree (`innerText`), where each element would otherwise recompute
  // every ancestor it inherits from. Nothing can change the cascade mid-read.
  let memo = null;

  function computedValues(element) {
    const remembered = memo?.get(element);
    if (remembered) return remembered;
    const values = new Map();
    const entries = declared(element);
    const names = new Set(entries.map((entry) => entry.name));
    // Which winning values are a shorthand awaiting substitution, so the part
    // for this longhand can be taken once the custom property resolves.
    const pending = new Map();
    for (const name of names) {
      const won = winner(entries.filter((entry) => entry.name === name));
      if (!won) continue;
      values.set(name, won.value);
      if (won.shorthand) pending.set(name, won.shorthand);
    }
    // Inheritance, then the explicit keywords that ask for it.
    const parent = flatParent(element);
    const inherited = parent ? computedValues(parent) : new Map();
    for (const name of INHERITED) {
      const own = values.get(name);
      if (own === undefined || own === "inherit") {
        const from = inherited.get(name);
        if (from !== undefined) values.set(name, from);
        else if (own === "inherit") values.delete(name);
      }
    }
    for (const [name, value] of values) {
      if (value !== "inherit") continue;
      const from = inherited.get(name);
      if (from === undefined) values.delete(name);
      else values.set(name, from);
    }
    // A custom property inherits whatever its name is.
    for (const [name, value] of inherited) {
      if (name.startsWith("--") && !values.has(name)) values.set(name, value);
    }
    // After inheritance, so a custom property an ancestor declared is in hand,
    // and before the initial values, which are what an invalid one falls to.
    for (const [name, value] of Array.from(values)) {
      if (!value.includes("var(")) continue;
      const resolved = substitute(value, values, new Set());
      if (resolved !== null && resolved.trim() !== "") {
        const shorthand = pending.get(name);
        if (shorthand === undefined) {
          values.set(name, resolved.trim());
          continue;
        }
        // The shorthand can be split now. If the substituted value does not
        // parse into this longhand, the declaration is invalid, like any other.
        const part = css.expandShorthand(shorthand, resolved).find(([longhand]) => longhand === name);
        if (part) {
          values.set(name, part[1]);
          continue;
        }
      }
      values.delete(name);
      const from = inherited.get(name);
      if (INHERITED.has(name) && from !== undefined) values.set(name, from);
    }
    // Colours resolve last, and `color` before the rest: `currentcolor`
    // anywhere else means whatever `color` ended up being, and `color:
    // currentcolor` means the inherited one.
    const declaredColor = colors === null ? undefined : values.get("color");
    if (declaredColor !== undefined) {
      values.set(
        "color",
        String(declaredColor).toLowerCase() === "currentcolor"
          ? inherited.get("color") ?? INITIAL.get("color")
          : colors.computedColor(declaredColor, inherited.get("color") ?? INITIAL.get("color")),
      );
    }
    if (colors !== null) {
      const currentColor = values.get("color") ?? INITIAL.get("color");
      for (const name of colors.COLOR_PROPERTIES) {
        if (name === "color") continue;
        const value = values.get(name);
        if (value !== undefined) values.set(name, colors.computedColor(value, currentColor));
      }
    }

    // Lengths, in pixels, before anything else reads one: `line-height: 1.5`
    // needs the font size, and the font size needs the parent's.
    const parentFontSize = Number.parseFloat(inherited.get("font-size") ?? "") || ROOT_FONT_SIZE;
    const rootFontSize = () => {
      const root = element.ownerDocument?.documentElement;
      if (!root || root === element) return ROOT_FONT_SIZE;
      return Number.parseFloat(computedValues(root).get("font-size") ?? "") || ROOT_FONT_SIZE;
    };
    const fontSize = resolveFontSize(values.get("font-size"), parentFontSize, rootFontSize());
    values.set("font-size", printPx(fontSize));
    const lineHeight = resolveLineHeight(values.get("line-height"), fontSize, rootFontSize());
    if (lineHeight !== undefined) values.set("line-height", lineHeight);
    // And every other length the table says a property takes: `em` against this
    // element's font size, `rem` against the root's, an absolute unit against
    // the pixel. A percentage, `ex`, `ch` and the viewport units are left as
    // written, because those need a box, a font or a window this DOM is not
    // measuring.
    for (const [name, value] of values) {
      if (name === "font-size" || name === "line-height" || name.startsWith("--")) continue;
      if (!types?.get(name)?.kinds.has("length")) continue;
      if (!/\d(?:px|em|rem|pt|pc|in|cm|mm|q)\b/i.test(value)) continue;
      const converted = String(value).split(/\s+/).map((component) => {
        const pixels = toPixels(component, fontSize, rootFontSize());
        return pixels === null ? component : printPx(pixels);
      }).join(" ");
      values.set(name, converted);
    }
    // A border with no style has no width, whatever was asked for — the one
    // used-value rule that needs nothing measured.
    for (const side of ["top", "right", "bottom", "left"]) {
      const style = values.get(`border-${side}-style`) ?? INITIAL.get(`border-${side}-style`);
      if (style === "none" || style === "hidden") values.set(`border-${side}-width`, "0px");
    }

    // The one keyword-to-number computation that needs no layout: a browser's
    // computed `font-weight` is always a number, and a test that sets `bold`
    // reads `700`. `bolder`/`lighter` are relative to the parent's and stay as
    // written, like every other value this DOM does not resolve.
    const weight = values.get("font-weight");
    if (weight === "normal") values.set("font-weight", "400");
    else if (weight === "bold") values.set("font-weight", "700");
    for (const [name, value] of INITIAL) {
      if (!values.has(name)) values.set(name, value);
    }
    // Blockification: a float, an absolutely positioned box, a flex or grid
    // item and the root element all compute to their block-level display.
    // Chrome reports it, and `innerText` breaks lines on it.
    const blockified = BLOCKIFY.get(values.get("display"));
    if (blockified && (values.get("float") !== "none"
      || ["absolute", "fixed"].includes(values.get("position"))
      || ["flex", "inline-flex", "grid", "inline-grid"].includes(inherited.get("display"))
      || element === element.ownerDocument?.documentElement)) {
      values.set("display", blockified);
    }
    memo?.set(element, values);
    return values;
  }

  // Built as rules rather than parsed from text: the user-agent sheet is
  // written in this file, so putting it through the CSS parser would only add a
  // dependency and a chance to disagree with itself.
  const uaRules = UA_RULES.map(([selector, declarations]) =>
    new CSSStyleRule(selector, Object.entries(declarations).map(([name, value]) => [name, value, false])));

  function getComputedStyle(element, pseudo = null) {
    if (!(element instanceof Element)) throw new TypeError("getComputedStyle expects an Element");
    // A pseudo-element has no box and no content here, so its computed style is
    // empty rather than wrong.
    if (pseudo !== null && pseudo !== undefined && String(pseudo) !== "") return css.readOnlyDeclaration([]);
    // An element outside the tree has no computed style at all — not the
    // initial values, nothing — which is what a browser answers for one.
    if (!element.isConnected) return css.readOnlyDeclaration([]);
    const values = computedValues(element);
    return css.readOnlyDeclaration(Array.from(values, ([name, value]) => [name, value, false]));
  }

  // `innerText` is the rendered-text collection the HTML specification defines
  // over computed values, not over boxes: which elements are rendered, which are
  // block-level, how white space collapses and how text is transformed are all
  // answered by the cascade above. So it is answered here rather than refused.
  // What a box alone would decide — a soft wrap, `::first-letter` — contributes
  // nothing to `innerText` in a browser either.
  const BLOCK_LEVEL = new Set(["block", "list-item", "table", "table-caption", "flex", "grid", "flow-root"]);
  // Elements whose children a browser renders inside a control, or not at all,
  // so contribute no text of their own.
  const NO_RENDERED_CHILDREN = new Set(["input", "textarea", "img", "canvas", "video", "audio", "iframe", "object", "embed", "meter", "progress"]);

  function renderedText(element) {
    // Not rendered — detached, or `display: none` here or above — reads its
    // text content, as the specification says.
    if (!element.checkVisibility()) return element.textContent;
    memo = new Map();
    try {
      const tokens = [];
      collectRendered(element, tokens, false);
      return joinRendered(tokens);
    } finally {
      memo = null;
    }
  }

  function collectRendered(node, tokens, inSelect) {
    for (const child of node.childNodes) {
      if (child instanceof tree.Text) {
        const values = computedValues(child.parentElement);
        if (values.get("visibility") !== "visible") continue;
        textTokens(child.data, values, inSelect, tokens);
        continue;
      }
      if (!(child instanceof Element)) continue;
      const values = computedValues(child);
      const display = values.get("display");
      if (display === "none") continue;
      const html = child.namespaceURI === HTML_NAMESPACE;
      if (html && child.localName === "br") {
        tokens.push({ text: "\n" });
        continue;
      }
      if (html && NO_RENDERED_CHILDREN.has(child.localName)) continue;
      const option = inSelect && html && (child.localName === "option" || child.localName === "optgroup");
      const breaks = option || BLOCK_LEVEL.has(display) ? (html && child.localName === "p" ? 2 : 1) : 0;
      if (breaks) tokens.push({ breaks });
      collectRendered(child, tokens, inSelect || (html && child.localName === "select"));
      if (display === "table-cell" && followingBox(child, "table-cell", child.parentElement)) tokens.push({ text: "\t" });
      if (display === "table-row" && followingBox(child, "table-row", enclosingTable(child))) tokens.push({ breaks: 1 });
      if (breaks) tokens.push({ breaks });
    }
  }

  // Whether another box of the same display follows this one inside `within`,
  // without descending into a nested table.
  function followingBox(element, display, within) {
    if (!within) return false;
    let seen = false;
    const walk = (parent) => {
      for (const child of parent.children) {
        if (child === element) { seen = true; continue; }
        const own = computedValues(child).get("display");
        if (own === "none") continue;
        if (seen && own === display) return true;
        if (own === "table") continue;
        if (walk(child)) return true;
      }
      return false;
    };
    return walk(within);
  }

  function enclosingTable(element) {
    for (let at = element.parentElement; at; at = at.parentElement) {
      if (computedValues(at).get("display") === "table") return at;
    }
    return null;
  }

  function textTokens(data, values, inSelect, tokens) {
    const mode = values.get("white-space") ?? "normal";
    // A select's options are drawn by the control, which does not take the
    // page's `text-transform`.
    const transform = inSelect ? "none" : values.get("text-transform") ?? "none";
    let text = data;
    if (transform === "uppercase") text = text.toUpperCase();
    else if (transform === "lowercase") text = text.toLowerCase();
    else if (transform === "capitalize") text = text.replace(/(^|[\s ])(\p{L})/gu, (_, gap, letter) => gap + letter.toUpperCase());
    if (mode === "pre" || mode === "pre-wrap" || mode === "break-spaces") {
      if (text) tokens.push({ text });
      return;
    }
    // `pre-line` keeps its line breaks and collapses the rest; everything else
    // collapses both. Only ASCII white space collapses — a no-break space is text.
    const pieces = mode === "pre-line"
      ? text.replace(/[ \t]*\n[ \t]*/g, "\n").split(/[ \t\r\f]+/)
      : text.split(/[ \t\n\r\f]+/);
    pieces.forEach((piece, index) => {
      if (index > 0) tokens.push({ space: true });
      if (piece) tokens.push({ text: piece });
    });
  }

  // Collapsible spaces merge, and vanish at the start and end of a line; runs of
  // required line breaks become the largest of them, and none survive at the
  // very start or end.
  function joinRendered(tokens) {
    const spaced = [];
    for (const token of tokens) {
      if (token.space && spaced.at(-1)?.space) continue;
      spaced.push(token);
    }
    const lineEdge = (token, end) => token === undefined || token.breaks
      || (token.text !== undefined && (end ? token.text.endsWith("\n") : token.text.startsWith("\n")));
    const kept = spaced.filter((token, index) =>
      !token.space || !(lineEdge(spaced[index - 1], true) || lineEdge(spaced[index + 1], false)));
    while (kept[0]?.breaks) kept.shift();
    while (kept.at(-1)?.breaks) kept.pop();
    let out = "";
    let pending = 0;
    for (const token of kept) {
      if (token.breaks) {
        pending = Math.max(pending, token.breaks);
        continue;
      }
      if (pending) out += "\n".repeat(pending);
      pending = 0;
      out += token.space ? " " : token.text;
    }
    return out;
  }

  // Setting `innerText` or `outerText`: the value's line breaks become `<br>`
  // elements and the rest text, replacing the children or the element itself.
  function replaceWithText(element, value, outer) {
    const document = element.ownerDocument;
    const fragment = document.createDocumentFragment();
    String(value).split(/\r\n|\r|\n/).forEach((line, index) => {
      if (index > 0) fragment.append(document.createElement("br"));
      if (line) fragment.append(document.createTextNode(line));
    });
    if (!outer) {
      element.replaceChildren(fragment);
      return;
    }
    const parent = element.parentNode;
    if (!parent) throw new DOMException("outerText can only be set on an element with a parent.", "NoModificationAllowedError");
    const previous = element.previousSibling;
    const next = element.nextSibling;
    if (fragment.childNodes.length === 0) fragment.append(document.createTextNode(""));
    element.replaceWith(fragment);
    // The text either side merges with what it now touches.
    const merge = (text) => {
      if (!(text instanceof tree.Text) || !(text.nextSibling instanceof tree.Text)) return;
      text.appendData(text.nextSibling.data);
      text.nextSibling.remove();
    };
    if (next?.previousSibling) merge(next.previousSibling);
    merge(previous);
  }

  // On the prototypes: every document has stylesheets, including one built by
  // `DOMParser` or `createHTMLDocument`.
  function install() {
    Object.defineProperty(Document.prototype, "styleSheets", {
      get() { return new StyleSheetList(documentSheets(this)); },
      configurable: true,
    });
    for (const target of [Document.prototype, ShadowRoot.prototype]) {
      Object.defineProperty(target, "adoptedStyleSheets", {
        get() { return this[ADOPTED] ?? []; },
        set(sheets) {
          const list = Array.from(sheets);
          for (const sheet of list) {
            if (!(sheet instanceof CSSStyleSheet)) throw new TypeError("adoptedStyleSheets takes CSSStyleSheet objects");
          }
          if (Object.hasOwn(this, ADOPTED)) this[ADOPTED] = list;
          else Object.defineProperty(this, ADOPTED, { value: list, writable: true, configurable: true });
        },
        configurable: true,
      });
    }
    // `offsetParent` is an algorithm over the tree and the computed `position`,
    // not a measurement, so it can be answered: nothing for an element that is
    // not rendered, otherwise the nearest positioned ancestor, table part, or
    // the body.
    Object.defineProperty(Element.prototype, "offsetParent", {
      get() {
        if (!this.isConnected || this.localName === "body" || this.localName === "html") return null;
        // Not rendered — its own `display: none` or an ancestor's — has no
        // offset parent, which is the walk `checkVisibility` already does.
        if (!this.checkVisibility()) return null;
        if (getComputedStyle(this).getPropertyValue("position") === "fixed") return null;
        for (let element = this.parentElement; element; element = element.parentElement) {
          if (element.localName === "body") return element;
          if (["td", "th", "table"].includes(element.localName)) return element;
          if (getComputedStyle(element).getPropertyValue("position") !== "static") return element;
        }
        return null;
      },
      configurable: true,
    });
    // The one layout question the cascade can answer: whether an element is
    // rendered at all. The geometry stays zero; `display: none` is knowable.
    Object.defineProperty(Element.prototype, "checkVisibility", {
      value(options = {}) {
        if (!this.isConnected) return false;
        for (let element = this; element instanceof Element; element = element.parentElement) {
          const computed = getComputedStyle(element);
          if (computed.getPropertyValue("display") === "none") return false;
          if ((options.checkVisibilityCSS || options.visibilityProperty)
            && ["hidden", "collapse"].includes(computed.getPropertyValue("visibility"))) return false;
          if ((options.checkOpacity || options.opacityProperty) && computed.getPropertyValue("opacity") === "0") return false;
        }
        return true;
      },
      writable: true,
      configurable: true,
    });
    Object.defineProperty(tree.HTMLElement.prototype, "innerText", {
      get() { return renderedText(this); },
      set(value) { replaceWithText(this, value, false); },
      configurable: true,
      enumerable: true,
    });
    Object.defineProperty(tree.HTMLElement.prototype, "outerText", {
      get() { return renderedText(this); },
      set(value) { replaceWithText(this, value, true); },
      configurable: true,
      enumerable: true,
    });
    Object.defineProperty(tree.HTMLStyleElement.prototype, "sheet", {
      get() { return styleSheetFor(this); },
      configurable: true,
    });
  }

  return {
    CSSStyleSheet, CSSRule, CSSRuleList, StyleSheetList, CSSStyleRule, CSSGroupingRule, CSSConditionRule,
    CSSMediaRule, CSSSupportsRule, CSSContainerRule, CSSLayerBlockRule, MediaList,
    getComputedStyle, supportsCondition, install,
  };
}
