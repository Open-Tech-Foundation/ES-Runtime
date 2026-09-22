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
  "overflow": "visible",
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
  "gap": "normal",
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
  "border-radius": "0px",
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
  "inset": "auto",
  "margin": "0px",
  "margin-top": "0px",
  "margin-right": "0px",
  "margin-bottom": "0px",
  "margin-left": "0px",
  "padding": "0px",
  "padding-top": "0px",
  "padding-right": "0px",
  "padding-bottom": "0px",
  "padding-left": "0px",
}));

// Where an element's own `style` attribute sits, above every author rule that
// is not `!important`.
const ORIGIN = { ua: 0, author: 1, inline: 2 };

export function createSheets({ tree, parse, selectors, css, mediaMatches, colors = null }) {
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
    // A declaration whose value the DOM cannot parse is dropped from the rule,
    // the way a browser drops it: the rest of the block still applies.
    if (kind === STYLE_RULE) return new CSSStyleRule(first, second.filter(([name, value]) => css.keepsDeclaration(name, value)));
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
    const collect = (rules, origin, scope = null) => {
      for (const { rule, order } of applicable(rules, origin, 0)) {
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
    if (expanded.length > 0) return [[name, value], ...expanded];
    // A shorthand written with `var()` cannot be split until the custom
    // property is substituted, which happens when the value is computed. Its
    // longhands take the whole text now and are expanded then — the
    // specification's "pending substitution value", under a plainer name.
    if (String(value).includes("var(")) {
      const longhands = css.shorthandLonghands(name);
      if (longhands.length > 0) return [[name, value], ...longhands.map((longhand) => [longhand, value, name])];
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

  function computedValues(element) {
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
    Object.defineProperty(tree.HTMLStyleElement.prototype, "sheet", {
      get() { return styleSheetFor(this); },
      configurable: true,
    });
  }

  return { CSSStyleSheet, CSSRule, CSSRuleList, StyleSheetList, CSSStyleRule, CSSGroupingRule, getComputedStyle, supportsCondition, install };
}
