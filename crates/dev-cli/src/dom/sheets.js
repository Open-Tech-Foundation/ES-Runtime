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
  ["html, body, div, p, h1, h2, h3, h4, h5, h6, ol, ul, li, dl, dt, dd, figure, figcaption, main, header, footer, section, article, aside, nav, address, blockquote, pre, hr, form, fieldset, legend, details, summary, dialog, search, hgroup, menu", { display: "block" }],
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
  ["head, link, meta, style, script, title, template, base, param, source, track, area", { display: "none" }],
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
  ["center, caption, th", { "text-align": "center" }],
  ["ul, ol", { "list-style-type": "disc" }],
  ["ol", { "list-style-type": "decimal" }],
];

// Initial values, for the properties whose initial value is a keyword a test can
// assert on. A length or a colour that a browser resolves against layout or a
// colour scheme is deliberately absent: answering `""` is honest, answering a
// number this DOM did not compute would not be.
const INITIAL = new Map([
  ["display", "inline"],
  ["visibility", "visible"],
  ["direction", "ltr"],
  ["font-style", "normal"],
  ["font-weight", "400"],
  ["font-variant", "normal"],
  ["text-align", "start"],
  ["text-transform", "none"],
  ["white-space", "normal"],
  ["list-style-position", "outside"],
  ["border-collapse", "separate"],
  ["pointer-events", "auto"],
  ["position", "static"],
  ["color", "rgb(0, 0, 0)"],
]);

// Where an element's own `style` attribute sits, above every author rule that
// is not `!important`.
const ORIGIN = { ua: 0, author: 1, inline: 2 };

export function createSheets({ tree, parse, selectors, css, mediaMatches }) {
  const { Document, Element, ShadowRoot, HTML_NAMESPACE } = tree;

  class CSSRuleList {
    constructor(rules) {
      Object.defineProperty(this, RULES, { value: rules });
      for (const [index, rule] of rules.entries()) Object.defineProperty(this, index, { value: rule, enumerable: true });
    }
    get length() { return this[RULES].length; }
    item(index) { return this[RULES][Number(index)] ?? null; }
    [Symbol.iterator]() { return this[RULES][Symbol.iterator](); }
  }

  class StyleSheetList {
    constructor(sheets) {
      Object.defineProperty(this, RULES, { value: sheets });
      for (const [index, sheet] of sheets.entries()) Object.defineProperty(this, index, { value: sheet, enumerable: true });
    }
    get length() { return this[RULES].length; }
    item(index) { return this[RULES][Number(index)] ?? null; }
    [Symbol.iterator]() { return this[RULES][Symbol.iterator](); }
  }

  class CSSRule {
    get cssText() { return ""; }
  }

  class CSSStyleRule extends CSSRule {
    constructor(selectorText, declarations) {
      super();
      this.selectorText = selectorText;
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

  class CSSGroupingRule extends CSSRule {
    constructor(name, conditionText, rules) {
      super();
      this.conditionText = conditionText;
      Object.defineProperty(this, "name", { value: name });
      this.cssRules = new CSSRuleList(rules);
    }
    get cssText() {
      return `@${this.name} ${this.conditionText} { ${Array.from(this.cssRules, (rule) => rule.cssText).join(" ")} }`;
    }
  }

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
    if (kind === STYLE_RULE) return new CSSStyleRule(first, second);
    if (kind === GROUP_RULE) return new CSSGroupingRule(first, second, children.map(ruleFrom));
    if (kind === OTHER_RULE) return new CSSOtherRule(first, second);
    throw new TypeError(`Unsupported CSS rule kind: ${kind}`);
  }

  class CSSStyleSheet {
    constructor(options = {}) {
      Object.defineProperty(this, RULES, { value: { rules: [], version: 0 } });
      this.media = String(options.media ?? "");
      this.disabled = false;
      this.cssRules = new CSSRuleList([]);
      Object.defineProperty(this, OWNER, { value: { node: null }, writable: true });
    }
    get ownerNode() { return this[OWNER].node; }
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
    if (rule.name === "media") return mediaMatches(rule.conditionText);
    if (rule.name === "supports") return supportsCondition(rule.conditionText);
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
  function* applicable(rules, origin, start) {
    let order = start;
    for (const rule of rules) {
      if (rule instanceof CSSGroupingRule) {
        if (conditionHolds(rule)) yield* applicable(Array.from(rule.cssRules), origin, order);
        continue;
      }
      if (!(rule instanceof CSSStyleRule)) continue;
      order += 1;
      yield { rule, origin, order };
    }
  }

  function matchesRule(element, rule) {
    // A pseudo-element rule styles something that is not this element, and a
    // selector this engine cannot match contributes nothing.
    if (rule.selectorText.includes("::")) return false;
    try {
      return selectors.matches(element, rule.selectorText);
    } catch {
      return false;
    }
  }

  function declared(element) {
    const document = element.ownerDocument;
    const root = element.getRootNode();
    const entries = [];
    const collect = (rules, origin) => {
      for (const { rule, order } of applicable(rules, origin, 0)) {
        if (!matchesRule(element, rule)) continue;
        const specificity = selectors.specificity(rule.selectorText);
        for (const [name, value, important] of rule[RULES]) {
          entries.push({ name: property(name), value, important, origin, specificity, order });
        }
      }
    };
    collect(uaRules, ORIGIN.ua);
    // A shadow root's sheets apply inside it; the document's apply outside.
    for (const sheet of root instanceof ShadowRoot ? documentSheets(root) : documentSheets(document)) {
      collect(sheetRules(sheet), ORIGIN.author);
    }
    for (const [name, entry] of element.style ? inlineEntries(element) : []) {
      entries.push({ name, value: entry.value, important: entry.priority === "important", origin: ORIGIN.inline, specificity: [0, 0, 0], order: 0 });
    }
    return entries;
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

  function computedValues(element) {
    const values = new Map();
    const entries = declared(element);
    const names = new Set(entries.map((entry) => entry.name));
    for (const name of names) {
      const won = winner(entries.filter((entry) => entry.name === name));
      if (won) values.set(name, won.value);
    }
    // Inheritance, then the explicit keywords that ask for it.
    const parent = element.parentElement;
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
    for (const [name, value] of INITIAL) {
      if (!values.has(name)) values.set(name, value);
    }
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

  function install(document) {
    Object.defineProperty(document, "styleSheets", {
      get() { return new StyleSheetList(documentSheets(this)); },
    });
    for (const target of [document, ShadowRoot.prototype]) {
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
    Object.defineProperty(tree.HTMLStyleElement.prototype, "sheet", {
      get() { return styleSheetFor(this); },
      configurable: true,
    });
  }

  return { CSSStyleSheet, CSSRule, CSSRuleList, StyleSheetList, CSSStyleRule, CSSGroupingRule, getComputedStyle, supportsCondition, install };
}
