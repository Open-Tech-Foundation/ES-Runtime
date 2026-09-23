// Layout-free DOM behavior shared by esdev, jsdom, and happy-dom. Every case
// returns JSON data rather than asserting so the runner can compare runtimes.

function reset(document) {
  document.body.replaceChildren();
}

// A name no other case has used, so a registry that cannot forget a definition
// does not make the next case fail.
let counter = 0;
function unique(prefix) {
  counter += 1;
  return `matrix-${prefix}-${counter}`;
}

function errorName(callback) {
  try {
    callback();
    return null;
  } catch (error) {
    return error.name;
  }
}

export const cases = [
  {
    group: "tree",
    name: "append-and-replace-children",
    run(window) {
      const { document } = window;
      reset(document);
      const parent = document.createElement("div");
      parent.append("one", document.createElement("i"));
      parent.replaceChildren("two", document.createElement("b"));
      return [
        parent.textContent,
        parent.children.length,
        parent.firstElementChild.localName,
      ];
    },
  },
  {
    group: "tree",
    name: "fragment-contents-move-on-append",
    run(window) {
      const { document } = window;
      reset(document);
      const fragment = document.createDocumentFragment();
      fragment.append(document.createElement("i"), document.createElement("b"));
      document.body.append(fragment);
      return [
        fragment.childNodes.length,
        document.body.children.length,
        document.body.innerHTML,
      ];
    },
  },
  {
    group: "tree",
    name: "deep-clone-is-independent",
    run(window) {
      const { document } = window;
      reset(document);
      const original = document.createElement("div");
      original.innerHTML = "<span>one</span>";
      const clone = original.cloneNode(true);
      clone.firstElementChild.textContent = "two";
      return [
        original.textContent,
        clone.textContent,
        clone.firstElementChild !== original.firstElementChild,
      ];
    },
  },
  {
    group: "tree",
    name: "collections-stay-live",
    run(window) {
      const { document } = window;
      reset(document);
      const parent = document.createElement("div");
      const children = parent.children;
      parent.appendChild(document.createElement("i"));
      parent.appendChild(document.createElement("b"));
      parent.firstElementChild.remove();
      return [children.length, children.item(0).localName];
    },
  },
  {
    group: "events",
    name: "capturing-and-bubbling-order",
    run(window) {
      const { document } = window;
      reset(document);
      const outer = document.createElement("div");
      const inner = document.createElement("button");
      outer.appendChild(inner);
      document.body.appendChild(outer);
      const order = [];
      outer.addEventListener("click", () => order.push("outer-capture"), true);
      inner.addEventListener("click", () => order.push("target"));
      outer.addEventListener("click", () => order.push("outer-bubble"));
      inner.click();
      return order;
    },
  },
  {
    group: "events",
    name: "cancelled-event-returns-false",
    run(window) {
      const { document, Event } = window;
      reset(document);
      const target = document.createElement("div");
      target.addEventListener("change", (event) => event.preventDefault());
      return target.dispatchEvent(new Event("change", { cancelable: true }));
    },
  },
  {
    group: "events",
    name: "inline-handlers-run-for-modern-events",
    run(window) {
      const { document } = window;
      const target = document.createElement("div");
      const calls = [];
      target.onclick = (received) => { calls.push(received.type); received.preventDefault(); };
      const event = new window.Event("click", { bubbles: true, cancelable: true });
      return [target.dispatchEvent(event), calls];
    },
  },
  {
    group: "events",
    name: "once-listener-runs-once",
    run(window) {
      const { document, Event } = window;
      reset(document);
      const target = document.createElement("div");
      let calls = 0;
      target.addEventListener("change", () => calls++, { once: true });
      target.dispatchEvent(new Event("change"));
      target.dispatchEvent(new Event("change"));
      return calls;
    },
  },
  {
    group: "events",
    name: "stopped-propagation-does-not-reach-parent",
    run(window) {
      const { document } = window;
      reset(document);
      const parent = document.createElement("div");
      const child = document.createElement("button");
      parent.appendChild(child);
      const seen = [];
      child.addEventListener("click", (event) => {
        seen.push("child");
        event.stopPropagation();
      });
      parent.addEventListener("click", () => seen.push("parent"));
      child.click();
      return seen;
    },
  },
  {
    group: "parsing",
    name: "fragment-serializes-text-and-attributes",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.innerHTML = '<p title="a&amp;b">one &lt; two</p>';
      return host.innerHTML;
    },
  },
  {
    group: "parsing",
    name: "raw-text-is-not-parsed-as-elements",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.innerHTML = "<script>one < two</script>";
      return [
        host.firstElementChild.localName,
        host.firstElementChild.textContent,
        host.querySelectorAll("i").length,
      ];
    },
  },
  {
    group: "parsing",
    name: "svg-attributes-and-self-closing-children",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        '<svg viewbox="0 0 10 10" gradientUnits="userSpaceOnUse"><circle cx="5" cy="5" r="4"/></svg>';
      const svg = document.body.firstElementChild;
      const circle = svg.firstElementChild;
      return [
        svg.getAttribute("viewBox"),
        svg.getAttribute("gradientUnits"),
        circle.localName,
        circle.getAttribute("r"),
      ];
    },
  },
  {
    group: "tree",
    name: "namespace-attribute-access-and-replacement",
    run(window) {
      const { document } = window;
      reset(document);
      const use = document.createElement("use");
      const xlink = "http://www.w3.org/1999/xlink";
      use.setAttribute("href", "plain");
      use.setAttributeNS(xlink, "xlink:href", "#first");
      use.setAttributeNS(xlink, "xlink:href", "#next");
      const clone = use.cloneNode();
      use.removeAttributeNS(xlink, "href");
      return [
        use.getAttributeNS(null, "href"),
        use.hasAttributeNS(xlink, "href"),
        clone.getAttributeNS(xlink, "href"),
        clone.attributes.length,
      ];
    },
  },
  {
    group: "tree",
    name: "dataset-reflects-data-attributes",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("article");
      element.setAttribute("data-user-id", "first");
      element.dataset.userId = "next";
      element.dataset.ready = "";
      const keys = Object.keys(element.dataset);
      delete element.dataset.ready;
      return [
        element.dataset.userId,
        element.getAttribute("data-user-id"),
        keys,
        element.hasAttribute("data-ready"),
      ];
    },
  },
  {
    group: "tree",
    name: "create-element-ns-preserves-identity",
    run(window) {
      const { document } = window;
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      const use = document.createElementNS("http://www.w3.org/2000/svg", "xlink:use");
      const html = document.createElementNS("http://www.w3.org/1999/xhtml", "DIV");
      return [svg.namespaceURI, svg.nodeName, use.prefix, use.localName, html.localName, html.tagName];
    },
  },
  {
    group: "parsing",
    name: "omitted-tags-html-allows-are-filled-in",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      const round = (source) => { host.innerHTML = source; return host.innerHTML; };
      return [
        round("<table><tr><td>x</td></tr></table>"),
        round("<table><thead><tr><td>h</td></tr></thead><tr><td>x</td></tr></table>"),
        round("<table><tr><td>a<td>b</table>"),
        round("<ul><li>a<li>b</ul>"),
        round("<ol><li>a<ul><li>b</ul><li>c</ol>"),
        round("<p>one<p>two"),
        round("<select><option>a<option>b</select>"),
        round("<dl><dt>t<dd>d</dl>"),
      ];
    },
  },
  {
    group: "tree",
    name: "table-collections-follow-the-section-order",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        "<table><caption>c</caption><thead><tr><th>h</th></tr></thead><tfoot><tr><td>f</td></tr></tfoot><tr><td>a</td><td>b</td></tr><tr><td>c</td></tr></table>";
      const table = document.body.firstElementChild;
      const row = table.rows[1];
      return [
        Array.from(table.rows, (entry) => entry.textContent),
        table.tBodies.length,
        table.caption.textContent,
        [row.cells.length, row.rowIndex, row.sectionRowIndex, row.cells[1].cellIndex],
        [table.tBodies[0].rows.length, table.tHead.rows.length],
        [document.createElement("tr").rowIndex, document.createElement("td").cellIndex],
      ];
    },
  },
  {
    group: "parsing",
    name: "fragment-structure-follows-its-context-element",
    run(window) {
      const { document } = window;
      reset(document);
      const table = document.createElement("table");
      table.innerHTML = "<tr><td>x</td></tr>";
      const body = document.createElement("tbody");
      body.innerHTML = "<tr><td>x</td></tr>";
      return [table.innerHTML, table.firstElementChild.localName, body.innerHTML, body.firstElementChild.localName];
    },
  },
  {
    group: "parsing",
    name: "malformed-markup-is-a-strict-esdev-limit",
    limit: "esdev rejects malformed HTML instead of applying browser recovery",
    expectedEsdev: { result: "SyntaxError" },
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      return errorName(() => {
        host.innerHTML = "<p><i>unclosed";
      });
    },
  },
  {
    group: "selectors",
    name: "compound-and-attribute-selector",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        '<section><a class="chosen" data-kind="x"></a><a data-kind="y"></a></section>';
      return document.querySelector("section > a.chosen[data-kind='x']")
        ?.getAttribute("data-kind") ?? null;
    },
  },
  {
    group: "selectors",
    name: "logical-selector-list",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        "<div class='a'></div><div class='b'></div><div class='c'></div>";
      return Array.from(
        document.querySelectorAll("div:is(.a, .b):not(.c)"),
        (element) => element.className,
      );
    },
  },
  {
    group: "selectors",
    name: "structural-selector",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        "<ul><li>one</li><li>two</li><li>three</li></ul>";
      return Array.from(
        document.querySelectorAll("li:nth-child(2n + 1)"),
        (element) => element.textContent,
      );
    },
  },
  {
    group: "selectors",
    name: "invalid-selector-throws-syntax-error",
    run(window) {
      const { document } = window;
      reset(document);
      return errorName(() => document.querySelector("div["));
    },
  },
  {
    group: "selectors",
    name: "nth-child-counts-only-the-of-list",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        "<ul><li class='x'>1</li><li>2</li><li class='x'>3</li><li class='x'>4</li><li>5</li><li class='x'>6</li></ul>";
      const list = document.body.firstElementChild;
      return [
        Array.from(list.querySelectorAll("li:nth-child(2n + 1 of .x)"), (item) => item.textContent),
        Array.from(list.querySelectorAll("li:nth-last-child(1 of .x)"), (item) => item.textContent),
        Array.from(list.querySelectorAll("li:nth-child(2n + 1)"), (item) => item.textContent),
      ];
    },
  },
  {
    group: "components",
    name: "defined-pseudo-class-follows-the-definition",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = "<matrix-widget></matrix-widget><p></p><matrix-absent></matrix-absent>";
      const before = Array.from(host.querySelectorAll(":defined"), (element) => element.localName);
      window.customElements.define("matrix-widget", class extends window.HTMLElement {});
      return [
        before,
        Array.from(host.querySelectorAll(":defined"), (element) => element.localName),
        Array.from(host.querySelectorAll(":not(:defined)"), (element) => element.localName),
        document.createElement("matrix-widget").matches(":defined"),
      ];
    },
  },
  {
    group: "tree",
    name: "adjacent-insertion-lands-at-four-positions",
    run(window) {
      const { document } = window;
      reset(document);
      const root = document.createElement("main");
      const target = document.createElement("p");
      root.append(target);
      document.body.append(root);
      target.insertAdjacentHTML("beforebegin", "<i>bb</i>");
      target.insertAdjacentHTML("afterbegin", "<b>ab</b>");
      target.insertAdjacentHTML("beforeend", "<u>be</u>");
      target.insertAdjacentHTML("afterend", "<s>ae</s>");
      const returned = target.insertAdjacentElement("afterbegin", document.createElement("em"));
      target.insertAdjacentText("beforeend", "text&");
      // An invalid position is deliberately not asserted here: happy-dom 20.0.11
      // hangs on one rather than throwing, and a case that never returns takes
      // the whole matrix with it. `crates/dev-cli/tests/cli.rs` covers it.
      return [root.innerHTML, returned.localName];
    },
  },
  {
    group: "components",
    name: "declarative-shadow-root-attaches-only-through-set-html-unsafe",
    run(window) {
      const { document } = window;
      reset(document);
      const markup = '<span><template shadowrootmode="open" shadowrootserializable=""><i>inner</i></template>light</span>';
      const inert = document.createElement("div");
      inert.innerHTML = markup;
      const attached = document.createElement("div");
      attached.setHTMLUnsafe(markup);
      const host = attached.firstElementChild;
      return [
        inert.firstElementChild.shadowRoot === null,
        inert.firstElementChild.firstElementChild.localName,
        host.shadowRoot.mode,
        host.shadowRoot.serializable,
        host.shadowRoot.innerHTML,
        attached.getHTML(),
        attached.getHTML({ serializableShadowRoots: true }),
      ];
    },
  },
  {
    group: "components",
    name: "form-associated-custom-element-joins-its-form",
    run(window) {
      const { document } = window;
      reset(document);
      const name = "matrix-field";
      if (!window.customElements.get(name)) {
        window.customElements.define(name, class extends window.HTMLElement {
          static formAssociated = true;
          constructor() {
            super();
            this.internals = this.attachInternals();
          }
        });
      }
      const form = document.createElement("form");
      const field = document.createElement(name);
      field.setAttribute("name", "chosen");
      form.append(field);
      document.body.append(form);
      field.internals.setFormValue("picked");
      const entries = Array.from(new window.FormData(form).entries());
      field.internals.setValidity({ valueMissing: true }, "pick something");
      const invalid = [field.internals.validity.valid, field.internals.validationMessage, form.checkValidity()];
      field.internals.setValidity({});
      return [field.internals.form === form, form.elements.length, entries, invalid, form.checkValidity()];
    },
  },
  {
    group: "components",
    name: "custom-state-matches-the-state-pseudo-class",
    run(window) {
      const { document } = window;
      reset(document);
      const name = "matrix-stateful";
      if (!window.customElements.get(name)) {
        window.customElements.define(name, class extends window.HTMLElement {
          constructor() {
            super();
            this.internals = this.attachInternals();
          }
        });
      }
      const host = document.createElement("div");
      document.body.append(host);
      const element = document.createElement(name);
      host.append(element);
      const before = host.querySelectorAll(`${name}:state(loading)`).length;
      element.internals.states.add("loading");
      const after = host.querySelectorAll(`${name}:state(loading)`).length;
      element.internals.states.delete("loading");
      return [before, after, host.querySelectorAll(`${name}:state(loading)`).length, element.internals.states.size];
    },
  },
  {
    group: "cascade",
    name: "origin-importance-specificity-and-order-decide-the-winner",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = document.createElement("style");
      sheet.textContent =
        "div { color: rgb(1, 1, 1) } .a { color: rgb(2, 2, 2) } #b { color: rgb(3, 3, 3) } .a { color: rgb(4, 4, 4) } .imp { color: rgb(5, 5, 5) !important }";
      document.head.append(sheet);
      const element = document.createElement("div");
      document.body.append(element);
      const colour = () => window.getComputedStyle(element).color;
      const seen = [colour()];
      element.className = "a";
      seen.push(colour());
      element.id = "b";
      seen.push(colour());
      element.style.color = "rgb(6, 6, 6)";
      seen.push(colour());
      element.classList.add("imp");
      seen.push(colour());
      element.style.setProperty("color", "rgb(7, 7, 7)", "important");
      seen.push(colour());
      sheet.remove();
      return seen;
    },
  },
  {
    group: "cascade",
    name: "inheritance-and-user-agent-defaults",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = document.createElement("style");
      sheet.textContent = ".parent { color: rgb(9, 9, 9); --brand: cyan; border-top-color: rgb(8, 8, 8) }";
      document.head.append(sheet);
      const parent = document.createElement("div");
      parent.className = "parent";
      const child = document.createElement("span");
      const strong = document.createElement("strong");
      const hidden = document.createElement("div");
      hidden.setAttribute("hidden", "");
      parent.append(child);
      document.body.append(parent, strong, hidden);
      const computed = window.getComputedStyle(child);
      const answer = [
        computed.color,
        computed.getPropertyValue("--brand"),
        computed.borderTopColor === window.getComputedStyle(parent).borderTopColor,
        [computed.display, computed.fontWeight, computed.visibility, computed.textAlign],
        window.getComputedStyle(parent).display,
        window.getComputedStyle(strong).fontWeight,
        window.getComputedStyle(hidden).display,
        window.getComputedStyle(document.createElement("li")).display,
      ];
      sheet.remove();
      return answer;
    },
  },
  {
    group: "cascade",
    name: "media-and-supports-conditions-gate-their-rules",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = document.createElement("style");
      sheet.textContent = [
        "@media (min-width: 100px) { .m { color: rgb(1, 2, 3) } }",
        "@media (min-width: 99999px) { .m { color: rgb(4, 5, 6) } }",
        "@media print { .m { font-weight: 900 } }",
        "@supports (display: grid) { .s { font-style: italic } }",
        "@supports not (display: grid) { .s { font-style: oblique } }",
      ].join("\n");
      document.head.append(sheet);
      const element = document.createElement("p");
      element.className = "m s";
      document.body.append(element);
      const computed = window.getComputedStyle(element);
      const answer = [
        computed.color,
        computed.fontWeight,
        computed.fontStyle,
        window.matchMedia("(min-width: 100px)").matches,
        window.matchMedia("(min-width: 99999px)").matches,
        window.matchMedia("print").matches,
        window.CSS.supports("color", "red"),
      ];
      sheet.remove();
      return answer;
    },
  },
  {
    group: "cascade",
    name: "constructed-sheets-apply-while-adopted",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = new window.CSSStyleSheet();
      sheet.replaceSync(".c { color: rgb(7, 8, 9) }");
      const element = document.createElement("div");
      element.className = "c";
      document.body.append(element);
      const before = window.getComputedStyle(element).color;
      document.adoptedStyleSheets = [sheet];
      const during = window.getComputedStyle(element).color;
      document.adoptedStyleSheets = [];
      return [
        sheet.cssRules.length,
        sheet.cssRules[0].selectorText,
        sheet.cssRules[0].style.getPropertyValue("color"),
        before,
        during,
        window.getComputedStyle(element).color,
      ];
    },
  },
  {
    group: "cascade",
    name: "a-shadow-root-is-styled-by-its-own-sheets",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<style>.in { color: rgb(2, 4, 6) }</style><p class='in'>x</p>";
      const inside = root.lastElementChild;
      const outside = document.createElement("p");
      outside.className = "in";
      document.body.append(outside);
      const scoped = new window.CSSStyleSheet();
      scoped.replaceSync(".in { font-weight: 700 }");
      root.adoptedStyleSheets = [scoped];
      return [
        window.getComputedStyle(inside).color,
        window.getComputedStyle(outside).color,
        window.getComputedStyle(inside).fontWeight,
      ];
    },
  },
  {
    group: "cascade",
    name: "an-equal-specificity-tie-goes-to-the-later-sheet",
    run(window) {
      const { document } = window;
      reset(document);
      // Two rules of equal specificity, in different sheets. The winner is the
      // one the document sees last: its `<style>` elements in tree order, then
      // its adopted sheets.
      const first = document.createElement("style");
      first.textContent = "span { border-top-width: 4px; border-top-style: solid }";
      document.head.append(first);
      const second = document.createElement("style");
      second.textContent = "span { border-top-width: 5px }";
      document.head.append(second);
      const adopted = new window.CSSStyleSheet();
      adopted.replaceSync("span { border-top-width: 3px }");

      const span = document.createElement("span");
      document.body.append(span);
      const width = () => window.getComputedStyle(span).borderTopWidth;
      const styleElementsOnly = width();
      document.adoptedStyleSheets = [adopted];
      const adoptedWins = width();

      // A shadow tree's own sheets are ordered the same way: this is the shape
      // Lit renders, where the static styles it adopts must beat the markup.
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<style>span { border-top-width: 4px; border-top-style: solid }</style><span></span>";
      const inner = root.lastElementChild;
      const scoped = new window.CSSStyleSheet();
      scoped.replaceSync("span { border-top-width: 3px }");
      const scopedBefore = window.getComputedStyle(inner).borderTopWidth;
      root.adoptedStyleSheets = [scoped];
      const scopedAfter = window.getComputedStyle(inner).borderTopWidth;

      document.adoptedStyleSheets = [];
      return [styleElementsOnly, adoptedWins, scopedBefore, scopedAfter];
    },
  },
  {
    group: "cascade",
    name: "nesting-resolves-against-its-parent-rule",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = document.createElement("style");
      sheet.textContent = ".card { color: rgb(1, 1, 1); & a { color: rgb(2, 2, 2) } b { color: rgb(3, 3, 3) } }";
      document.head.append(sheet);
      const card = document.createElement("div");
      card.className = "card";
      card.innerHTML = "<a>l</a><b>bold</b>";
      document.body.append(card);
      const answer = [
        window.getComputedStyle(card).color,
        window.getComputedStyle(card.firstElementChild).color,
        window.getComputedStyle(card.lastElementChild).color,
      ];
      sheet.remove();
      return answer;
    },
  },
  {
    group: "cascade",
    name: "geometry-answers-zero-and-rendering-is-knowable",
    limit: "esdev has no layout, so every measurement is zero",
    expectedEsdev: { result: ["DOMRect", 0, 0, 0, 0, true, [true, false, false], true] },
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      const hidden = document.createElement("div");
      hidden.style.display = "none";
      const inner = document.createElement("span");
      hidden.append(inner);
      document.body.append(element, hidden);
      const rect = element.getBoundingClientRect();
      element.scrollTop = 40;
      return [
        rect.constructor.name,
        rect.width,
        rect.height,
        element.offsetWidth,
        element.scrollTop,
        element.offsetParent === document.body,
        [element.checkVisibility(), hidden.checkVisibility(), inner.checkVisibility()],
        hidden.offsetParent === null && inner.offsetParent === null,
      ];
    },
  },
  {
    group: "tree",
    name: "html-names-are-case-insensitive",
    run(window) {
      const { document } = window;
      reset(document);
      const made = document.createElement("DIV");
      const host = document.createElement("div");
      host.innerHTML = "<SPAN CLASS=a ID=b>x</SPAN>";
      const svg = document.createElement("div");
      svg.innerHTML = "<svg viewBox='0 0 1 1'><linearGradient/></svg>";
      return [
        [made.localName, made.tagName],
        host.innerHTML,
        [host.firstElementChild.className, host.firstElementChild.id],
        svg.firstElementChild.getAttribute("viewBox"),
        svg.firstElementChild.firstElementChild.localName,
        document.createElementNS("http://www.w3.org/2000/svg", "linearGradient").localName,
      ];
    },
  },
  {
    group: "tree",
    name: "bare-constructors-and-fragment-lookups",
    run(window) {
      const { document } = window;
      reset(document);
      const fragment = new window.DocumentFragment();
      fragment.append(new window.Text("one"), new window.Comment("two"));
      const inside = document.createElement("b");
      inside.id = "in-fragment";
      fragment.append(inside);
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<p id='in-shadow'>x</p>";
      const target = document.createElement("div");
      document.body.append(target);
      target.append(fragment);
      return [
        target.innerHTML,
        fragment.getElementById === undefined ? "missing" : fragment.getElementById("in-fragment") === inside,
        root.getElementById === undefined ? "missing" : root.getElementById("in-shadow").localName,
        document.getElementById("in-shadow") === null,
        document.hasFocus(),
      ];
    },
  },
  {
    group: "forms",
    name: "a-radio-group-is-exclusive-on-the-checked-setter",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      form.innerHTML =
        "<input type=radio name=pick value=yes><input type=radio name=pick value=no><input type=radio name=other value=x>";
      document.body.append(form);
      const [yes, no, other] = form.querySelectorAll("input");
      yes.checked = true;
      no.checked = true;
      const afterBoth = [yes.checked, no.checked, other.checked];
      const entries = Array.from(new window.FormData(form).entries());
      no.checked = false;
      return [afterBoth, entries, [yes.checked, no.checked]];
    },
  },
  {
    group: "selectors",
    name: "escapes-in-an-identifier",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML =
        '<a id="id.with.dots">1</a><b class="foo:bar">2</b><u id="1leading">3</u><s class="caf\u00e9">4</s>';
      const text = (selector) => errorName(() => document.querySelector(selector)) ?? document.querySelector(selector)?.textContent ?? null;
      return [
        text("#id\\.with\\.dots"),
        text(".foo\\:bar"),
        text("#\\31 leading"),
        text(".caf\\e9"),
        text("#id\\.with\\.dots + .foo\\:bar"),
        document.querySelector("#id\\.with\\.dots").matches("#id\\.with\\.dots"),
      ];
    },
  },
  {
    group: "cascade",
    name: "an-unknown-property-is-not-a-declaration",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      document.body.append(element);
      element.style.color = "rgb(1, 2, 3)";
      const computed = window.getComputedStyle(element);
      return [
        [element.style.color, element.style.transform, element.style.nonsenseProp],
        [computed.transform, computed.nonsenseProp],
        ["transform" in computed, "nonsenseProp" in computed],
        [window.CSS.supports("display", "grid"), window.CSS.supports("nonsense-prop", "1")],
      ];
    },
  },
  {
    group: "traversal",
    name: "a-node-iterator-walks-and-steps-back",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.innerHTML = "<a>1</a><b><i>2</i></b><u>3</u>";
      document.body.append(host);
      const walk = (whatToShow, filter) => {
        const iterator = document.createNodeIterator(host, whatToShow, filter ?? null);
        const seen = [];
        for (let node = iterator.nextNode(); node; node = iterator.nextNode()) {
          seen.push(node.localName ?? node.data);
        }
        return seen;
      };
      const stepping = document.createNodeIterator(host, window.NodeFilter.SHOW_ELEMENT);
      const first = stepping.nextNode().localName;
      const second = stepping.nextNode().localName;
      const back = stepping.previousNode().localName;
      return [
        walk(window.NodeFilter.SHOW_ELEMENT),
        walk(window.NodeFilter.SHOW_TEXT),
        // A NodeIterator has no REJECT, so a rejected node is only skipped.
        walk(window.NodeFilter.SHOW_ELEMENT, (node) => node.localName === "i" ? 1 : 2),
        [first, second, back, stepping.pointerBeforeReferenceNode],
      ];
    },
  },
  {
    group: "tree",
    name: "a-secondary-document-is-a-whole-document",
    run(window) {
      const { document } = window;
      reset(document);
      const parsed = new window.DOMParser().parseFromString("<p>x</p>", "text/html");
      const made = document.implementation.createHTMLDocument("t");
      const xml = document.implementation.createDocument("http://www.w3.org/2000/svg", "svg", null);
      return [
        [typeof parsed.createRange, parsed.createRange().collapsed, parsed.styleSheets.length],
        [typeof made.createRange, typeof made.createNodeIterator, made.styleSheets.length],
        [xml.documentElement.localName, xml.documentElement.namespaceURI, xml.doctype],
        "cookie" in document,
      ];
    },
  },
  {
    group: "components",
    name: "reaction-order-on-creation-and-connection",
    run(window) {
      const { document } = window;
      reset(document);
      const log = [];
      const name = unique("order");
      window.customElements.define(name, class extends window.HTMLElement {
        static get observedAttributes() { return ["value"]; }
        constructor() { super(); log.push("constructed"); }
        connectedCallback() { log.push(`connected:${this.isConnected}`); }
        disconnectedCallback() { log.push("disconnected"); }
        attributeChangedCallback(attribute, before, after) { log.push(`attribute:${attribute}:${before}:${after}`); }
      });
      const element = document.createElement(name);
      log.push("created");
      element.setAttribute("value", "one");
      element.setAttribute("other", "ignored");
      document.body.append(element);
      element.setAttribute("value", "two");
      element.remove();
      return log;
    },
  },
  {
    group: "components",
    name: "an-existing-element-upgrades-when-it-is-defined",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("late");
      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = `<${name} value=a></${name}><div><${name} value=b></${name}></div>`;
      const before = [host.firstElementChild.constructor.name, typeof host.firstElementChild.upgraded];
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        static get observedAttributes() { return ["value"]; }
        constructor() { super(); this.upgraded = true; }
        connectedCallback() { log.push(`connected:${this.getAttribute("value")}`); }
        attributeChangedCallback(attribute, old, next) { log.push(`attribute:${next}`); }
      });
      return [before, log, host.firstElementChild.upgraded === true, host.querySelectorAll(`${name}:defined`).length];
    },
  },
  {
    group: "components",
    name: "parsed-children-upgrade-in-tree-order",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("parsed");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        connectedCallback() { log.push(this.id); }
      });
      const host = document.createElement("div");
      document.body.append(host);
      host.innerHTML = `<${name} id=outer><${name} id=inner></${name}></${name}><${name} id=last></${name}>`;
      return [log, host.querySelectorAll(name).length];
    },
  },
  {
    group: "components",
    name: "the-registry-answers-about-a-definition",
    async run(window) {
      const { document } = window;
      reset(document);
      const name = unique("registry");
      const pending = window.customElements.whenDefined(name);
      const Defined = class extends window.HTMLElement {};
      const before = [window.customElements.get(name), typeof window.customElements.whenDefined];
      window.customElements.define(name, Defined);
      const resolved = await pending;
      return [
        before,
        window.customElements.get(name) === Defined,
        resolved === Defined,
        typeof window.customElements.getName === "function" ? window.customElements.getName(Defined) : "missing",
        errorName(() => window.customElements.define(name, class extends window.HTMLElement {})),
        errorName(() => window.customElements.define(unique("other"), Defined)),
        errorName(() => window.customElements.define("nodash", class extends window.HTMLElement {})),
      ];
    },
  },
  {
    group: "components",
    name: "an-element-can-be-upgraded-on-demand",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("ondemand");
      const detached = document.createElement("div");
      detached.innerHTML = `<${name}></${name}>`;
      const element = detached.firstElementChild;
      window.customElements.define(name, class extends window.HTMLElement {
        constructor() { super(); this.upgraded = true; }
      });
      const before = element.upgraded ?? null;
      window.customElements.upgrade(detached);
      return [before, element.upgraded === true, element.matches(":defined")];
    },
  },
  {
    group: "components",
    name: "slots-assign-and-flatten",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.innerHTML = "<p slot=head>H</p><span>light</span><p slot=head>H2</p>";
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<slot name=head></slot><slot><i>fallback</i></slot>";
      const [named, unnamed] = root.querySelectorAll("slot");
      return [
        Array.from(named.assignedNodes(), (node) => node.textContent),
        Array.from(unnamed.assignedElements(), (node) => node.localName),
        host.firstElementChild.assignedSlot === named,
        Array.from(unnamed.assignedNodes({ flatten: true }), (node) => node.localName ?? node.nodeName),
        Array.from(root.querySelector("slot[name=head]").assignedElements(), (node) => node.textContent),
      ];
    },
  },
  {
    group: "components",
    name: "flattening-recurses-through-a-slotted-slot",
    run(window) {
      const { document } = window;
      reset(document);
      // Two components nested: the outer one's slot is a light child of the
      // inner one, so it is itself a slottable. Flattening has to replace it
      // with what it assigns, however deep that goes.
      const host = document.createElement("div");
      document.body.append(host);
      const outer = host.attachShadow({ mode: "open" });
      outer.innerHTML = "<div id=inner><slot id=outer-slot><em>outer fallback</em></slot></div>";
      const inner = outer.getElementById("inner").attachShadow({ mode: "open" });
      inner.innerHTML = "<slot id=inner-slot><em>inner fallback</em></slot>";
      const innerSlot = inner.querySelector("slot");
      const outerSlot = outer.querySelector("slot");
      const names = (slot, options) =>
        Array.from(slot.assignedNodes(options), (node) => node.localName ?? node.nodeName);

      const assignedToInner = names(innerSlot);
      const emptyFlatten = names(innerSlot, { flatten: true });
      const outerEmptyFlatten = names(outerSlot, { flatten: true });
      host.append(document.createElement("i"), document.createElement("b"));
      return [
        assignedToInner,
        emptyFlatten,
        outerEmptyFlatten,
        names(innerSlot, { flatten: true }),
        names(outerSlot, { flatten: true }),
        names(outerSlot),
      ];
    },
  },
  {
    group: "components",
    name: "a-slot-reports-a-change",
    async run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<slot></slot>";
      const slot = root.firstElementChild;
      const changes = [];
      slot.addEventListener("slotchange", (event) => changes.push(`${event.type}:${event.bubbles}:${event.composed}`));
      host.append(document.createElement("i"));
      await settled();
      const afterAdd = changes.length;
      host.firstElementChild.remove();
      await settled();
      return [afterAdd, changes.length, changes[0] ?? null];
    },
  },
  {
    group: "components",
    name: "internals-reflect-aria-and-form-state",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("aria");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        static formAssociated = true;
        constructor() {
          super();
          this.internals = this.attachInternals();
        }
        formDisabledCallback(disabled) { log.push(`disabled:${disabled}`); }
        formResetCallback() { log.push("reset"); }
      });
      const form = document.createElement("form");
      const fieldset = document.createElement("fieldset");
      const element = document.createElement(name);
      fieldset.append(element);
      form.append(fieldset);
      document.body.append(form);
      const internals = element.internals;
      internals.role = "checkbox";
      internals.ariaLabel = "Pick one";
      fieldset.disabled = true;
      form.reset();
      return [
        [internals.role ?? "missing", internals.ariaLabel ?? "missing"],
        [element.getAttribute("role"), element.getAttribute("aria-label")],
        log,
      ];
    },
  },
  {
    group: "components",
    name: "a-clonable-shadow-root-is-cloned",
    run(window) {
      const { document } = window;
      reset(document);
      const clonable = document.createElement("div");
      clonable.attachShadow({ mode: "open", clonable: true }).innerHTML = "<i>inside</i>";
      const plain = document.createElement("div");
      plain.attachShadow({ mode: "open" }).innerHTML = "<i>inside</i>";
      document.body.append(clonable, plain);
      const clonedClonable = clonable.cloneNode(true);
      const clonedPlain = plain.cloneNode(true);
      return [
        clonedClonable.shadowRoot === null ? "no root" : clonedClonable.shadowRoot.innerHTML,
        clonedPlain.shadowRoot === null ? "no root" : clonedPlain.shadowRoot.innerHTML,
      ];
    },
  },
  {
    group: "components",
    name: "shadow-styles-reach-the-host-and-the-slotted",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.className = "card";
      host.innerHTML = "<p>light</p>";
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML =
        "<style>:host { color: rgb(1, 1, 1) } :host(.card) { font-weight: 700 } ::slotted(p) { font-style: italic } i { color: rgb(2, 2, 2) }</style><slot></slot><i>in</i>";
      const inner = root.querySelector("i");
      const slotted = host.firstElementChild;
      return [
        window.getComputedStyle(host).color,
        window.getComputedStyle(host).fontWeight,
        window.getComputedStyle(slotted).fontStyle,
        window.getComputedStyle(inner).color,
      ];
    },
  },
  {
    group: "components",
    name: "a-constructor-is-refused-when-it-misbehaves",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("bad");
      window.customElements.define(name, class extends window.HTMLElement {});
      const Defined = window.customElements.get(name);
      const direct = new Defined();
      return [
        direct.localName,
        direct.isConnected,
        errorName(() => window.customElements.define(unique("notaclass"), {})),
        errorName(() => document.createElement(unique("undefinedname")).localName),
      ];
    },
  },
  {
    group: "components",
    name: "an-element-moved-between-documents-is-adopted",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("adopted");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        adoptedCallback(from, to) { log.push(`adopted:${from === to}`); }
        connectedCallback() { log.push("connected"); }
        disconnectedCallback() { log.push("disconnected"); }
      });
      const element = document.createElement(name);
      document.body.append(element);
      const other = document.implementation.createHTMLDocument("other");
      other.body.append(other.adoptNode(element));
      return [log, element.ownerDocument === other, element.isConnected];
    },
  },
  {
    group: "components",
    name: "attributes-report-their-namespace-and-old-value",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("attrs");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        static get observedAttributes() { return ["value", "href"]; }
        attributeChangedCallback(attribute, before, after, namespace) {
          log.push([attribute, before, after, namespace ?? null].join("|"));
        }
      });
      const element = document.createElement(name);
      element.setAttribute("value", "one");
      element.setAttribute("value", "two");
      element.removeAttribute("value");
      element.setAttributeNS("http://www.w3.org/1999/xlink", "xlink:href", "#a");
      element.toggleAttribute("value");
      return log;
    },
  },
  {
    group: "components",
    name: "a-shadow-host-refuses-a-second-root-and-the-wrong-element",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.attachShadow({ mode: "open" });
      return [
        errorName(() => host.attachShadow({ mode: "open" })),
        errorName(() => document.createElement("input").attachShadow({ mode: "open" })),
        errorName(() => document.createElement("div").attachShadow({ mode: "sideways" })),
        errorName(() => document.createElement("span").attachShadow({ mode: "open" })),
      ];
    },
  },
  {
    group: "components",
    name: "focus-inside-a-root-is-reported-from-both-sides",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<input>";
      const inner = root.firstElementChild;
      inner.focus();
      return [
        document.activeElement === host,
        root.activeElement === inner,
        document.activeElement?.localName ?? null,
      ];
    },
  },
  {
    group: "components",
    name: "an-event-is-retargeted-and-a-closed-root-hides-its-path",
    run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "closed" });
      root.innerHTML = "<button></button>";
      const inner = root.firstElementChild;
      const seen = [];
      document.body.addEventListener("composed-probe", (event) => {
        seen.push([event.target.localName, event.composedPath().length]);
      });
      inner.dispatchEvent(new window.Event("composed-probe", { bubbles: true, composed: true }));
      const uncomposed = [];
      document.body.addEventListener("scoped-probe", () => uncomposed.push("escaped"));
      inner.dispatchEvent(new window.Event("scoped-probe", { bubbles: true }));
      return [seen, uncomposed.length];
    },
  },
  {
    group: "components",
    name: "aria-reflects-through-attributes-and-internals",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      document.body.append(element);
      element.role = "button";
      element.ariaLabel = "Save";
      element.ariaHidden = "true";
      const reflected = [element.getAttribute("role"), element.getAttribute("aria-label"), element.getAttribute("aria-hidden")];
      element.setAttribute("aria-label", "Changed");
      return [reflected, element.ariaLabel, element.role];
    },
  },
  {
    group: "components",
    name: "a-slot-change-follows-the-slot-attribute",
    async run(window) {
      const { document } = window;
      reset(document);
      const host = document.createElement("div");
      host.innerHTML = "<p>one</p>";
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      root.innerHTML = "<slot name=a></slot><slot></slot>";
      const [named, unnamed] = root.querySelectorAll("slot");
      await settled();
      const changes = [];
      named.addEventListener("slotchange", () => changes.push("named"));
      unnamed.addEventListener("slotchange", () => changes.push("unnamed"));
      host.firstElementChild.slot = "a";
      await settled();
      return [changes, named.assignedNodes().length, unnamed.assignedNodes().length];
    },
  },
  {
    group: "components",
    name: "a-template-holds-a-component-and-clones-it",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("templated");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        connectedCallback() { log.push("connected"); }
      });
      const template = document.createElement("template");
      template.innerHTML = `<${name}></${name}>`;
      const inert = [log.length, template.content.firstElementChild.matches(":defined")];
      document.body.append(template.content.cloneNode(true));
      return [inert, log, document.body.firstElementChild.matches(":defined")];
    },
  },
  {
    group: "components",
    name: "adoption-and-a-shadow-sweep-report-once",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("adopt");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        connectedCallback() { log.push("connected"); }
        disconnectedCallback() { log.push("disconnected"); }
        adoptedCallback() { log.push("adopted"); }
      });
      const element = document.createElement(name);
      document.body.append(element);
      log.length = 0;
      document.adoptNode(element);
      const same = log.slice();
      log.length = 0;
      const other = document.implementation.createHTMLDocument("other");
      other.body.append(other.adoptNode(element));
      const across = log.slice();
      // A definition arriving late reaches inside a shadow root too.
      const host = document.createElement("div");
      document.body.append(host);
      const root = host.attachShadow({ mode: "open" });
      const late = unique("shadow");
      root.innerHTML = `<${late}></${late}>`;
      const inside = root.firstElementChild;
      window.customElements.define(late, class extends window.HTMLElement {
        constructor() { super(); this.upgraded = true; }
      });
      return [same, across, [inside.upgraded === true, inside.matches(":defined")]];
    },
  },
  {
    group: "components",
    name: "a-closed-root-serializes-when-it-is-asked-for",
    run(window) {
      const { document } = window;
      reset(document);
      const serializable = document.createElement("div");
      serializable.attachShadow({ mode: "closed", serializable: true }).innerHTML = "<i>s</i>";
      const named = document.createElement("div");
      const namedRoot = named.attachShadow({ mode: "closed" });
      namedRoot.innerHTML = "<b>n</b>";
      document.body.append(serializable, named);
      return [
        document.body.getHTML(),
        document.body.getHTML({ serializableShadowRoots: true }).includes("<i>s</i>"),
        document.body.getHTML({ shadowRoots: [namedRoot] }).includes("<b>n</b>"),
        document.body.getHTML({ serializableShadowRoots: true }).includes("<b>n</b>"),
      ];
    },
  },
  {
    group: "components",
    name: "a-reset-clears-a-custom-elements-value",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("resettable");
      window.customElements.define(name, class extends window.HTMLElement {
        static formAssociated = true;
        constructor() { super(); this.internals = this.attachInternals(); }
      });
      const form = document.createElement("form");
      const field = document.createElement(name);
      field.setAttribute("name", "f");
      form.append(field);
      document.body.append(form);
      field.internals.setFormValue("changed");
      const before = Array.from(new window.FormData(form).entries());
      form.reset();
      return [before, Array.from(new window.FormData(form).entries())];
    },
  },
  {
    group: "components",
    name: "a-disabled-attribute-and-a-throwing-constructor",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("ownstate");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        static formAssociated = true;
        formDisabledCallback(state) { log.push(state); }
      });
      const element = document.createElement(name);
      document.body.append(element);
      element.setAttribute("disabled", "");
      element.removeAttribute("disabled");
      const thrower = unique("thrower");
      window.customElements.define(thrower, class extends window.HTMLElement {
        constructor() { super(); throw new Error("boom"); }
      });
      const made = errorName(() => document.createElement(thrower));
      const element2 = made === null ? document.createElement(thrower) : null;
      return [log, made, element2 === null ? "threw" : [element2.localName === thrower, element2.matches(":defined")]];
    },
  },
  {
    group: "tree",
    name: "moveBefore-keeps-state-and-connection",
    run(window) {
      const { document } = window;
      reset(document);
      if (typeof document.body.moveBefore !== "function") return "missing";
      const name = unique("moved");
      const log = [];
      window.customElements.define(name, class extends window.HTMLElement {
        connectedCallback() { log.push("connected"); }
        disconnectedCallback() { log.push("disconnected"); }
      });
      const from = document.createElement("div");
      const to = document.createElement("div");
      document.body.append(from, to);
      const element = document.createElement(name);
      const input = document.createElement("input");
      from.append(element, input);
      input.value = "kept";
      log.length = 0;
      to.moveBefore(element, null);
      const moved = [from.children.length, to.children.length, log.slice()];
      to.moveBefore(input, element);
      const detached = document.createElement("span");
      return [
        moved,
        [Array.from(to.children, (child) => child.localName), input.value, log.slice()],
        [errorName(() => to.moveBefore(detached, null)), errorName(() => to.moveBefore(element, detached))],
      ];
    },
  },
  {
    group: "parsing",
    name: "a-document-is-parsed-from-markup",
    run(window) {
      const { document } = window;
      reset(document);
      if (typeof window.Document.parseHTMLUnsafe !== "function") return "missing";
      const parsed = window.Document.parseHTMLUnsafe(
        '<p>one</p><div><template shadowrootmode="open"><i>in</i></template></div>',
      );
      return [
        parsed.body.firstElementChild.localName,
        parsed.body.children.length,
        parsed.body.lastElementChild.shadowRoot?.innerHTML ?? "no root",
        parsed.defaultView,
        typeof window.ShadowRoot.parseHTMLUnsafe,
      ];
    },
  },
  {
    group: "components",
    name: "a-dialog-opens-closes-and-returns",
    run(window) {
      const { document } = window;
      reset(document);
      const dialog = document.createElement("dialog");
      document.body.append(dialog);
      const log = [];
      dialog.addEventListener("close", () => log.push(`close:${dialog.returnValue}`));
      dialog.addEventListener("cancel", () => log.push("cancel"));
      const closed = [dialog.open, dialog.returnValue, dialog.matches(":modal")];
      dialog.show();
      const shown = [dialog.open, dialog.hasAttribute("open"), dialog.matches(":modal")];
      dialog.close("ok");
      const after = [dialog.open, dialog.returnValue];
      dialog.showModal();
      const modal = [dialog.open, dialog.matches(":modal")];
      dialog.close();
      return [closed, shown, after, modal, log, errorName(() => document.createElement("dialog").showModal())];
    },
  },
  {
    group: "components",
    name: "a-popover-toggles-and-reports-its-state",
    run(window) {
      const { document } = window;
      reset(document);
      const popover = document.createElement("div");
      popover.setAttribute("popover", "");
      popover.id = "matrix-popover";
      document.body.append(popover);
      const log = [];
      popover.addEventListener("beforetoggle", (event) => log.push(`before:${event.oldState}->${event.newState}`));
      popover.addEventListener("toggle", (event) => log.push(`toggle:${event.oldState}->${event.newState}`));
      const kinds = [popover.popover, document.createElement("div").popover];
      popover.showPopover();
      const open = popover.matches(":popover-open");
      popover.hidePopover();
      const closed = popover.matches(":popover-open");
      const toggled = [popover.togglePopover(), popover.matches(":popover-open")];
      popover.hidePopover();
      return [kinds, open, closed, toggled, log, errorName(() => document.createElement("div").showPopover())];
    },
  },
  {
    group: "components",
    name: "a-command-button-acts-on-the-element-it-names",
    run(window) {
      const { document } = window;
      reset(document);
      const popover = document.createElement("div");
      popover.setAttribute("popover", "");
      popover.id = "matrix-commanded";
      const button = document.createElement("button");
      button.setAttribute("command", "show-popover");
      button.setAttribute("commandfor", "matrix-commanded");
      document.body.append(popover, button);
      const seen = [];
      popover.addEventListener("command", (event) => {
        seen.push([event.type, event.command, event.source === button, typeof window.CommandEvent === "function" && event instanceof window.CommandEvent]);
      });
      button.click();
      const opened = popover.matches(":popover-open");
      popover.hidePopover();
      return [seen, opened];
    },
  },
  {
    group: "tree",
    name: "an-attribute-name-is-lowercased-for-html-only",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      document.body.append(element);
      element.setAttribute("tabIndex", 0);
      element.setAttribute("contentEditable", "true");
      const set = [element.getAttribute("tabindex"), element.tabIndex, element.hasAttribute("tabIndex"), element.getAttributeNames()];
      element.removeAttribute("contentEditable");
      const removed = [element.getAttribute("contenteditable"), element.outerHTML];
      const toggled = [element.toggleAttribute("HIDDEN"), element.hasAttribute("hidden")];
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("viewBox", "0 0 1 1");
      return [set, removed, toggled, [svg.getAttribute("viewBox"), svg.getAttribute("viewbox"), svg.getAttributeNames()]];
    },
  },
  {
    group: "tree",
    name: "a-document-has-no-text-content",
    run(window) {
      const { document } = window;
      reset(document);
      const before = document.textContent;
      document.textContent = "";
      return [before, document.documentElement?.tagName ?? null, document.body !== null, document.doctype?.name ?? null];
    },
  },
  {
    group: "tree",
    name: "internal-work-does-not-read-the-attributes-accessor",
    run(window) {
      const { document } = window;
      reset(document);
      const original = Object.getOwnPropertyDescriptor(window.Element.prototype, "attributes");
      let reads = 0;
      Object.defineProperty(window.Element.prototype, "attributes", {
        get() { reads += 1; return original.get.call(this); },
        configurable: true,
      });
      let html = "";
      let cloned = null;
      try {
        const element = document.createElement("span");
        document.body.append(element);
        element.setAttribute("a", "1");
        element.getAttribute("a");
        element.removeAttribute("a");
        element.setAttribute("b", "2");
        html = element.outerHTML;
        cloned = element.cloneNode(true).getAttribute("b");
      } finally {
        Object.defineProperty(window.Element.prototype, "attributes", original);
      }
      return [reads, html, cloned];
    },
  },
  {
    group: "forms",
    name: "input-indeterminate-is-non-reflecting-state",
    run(window) {
      const { document } = window;
      reset(document);
      const input = document.createElement("input");
      input.type = "checkbox";
      const initial = input.indeterminate;
      input.indeterminate = true;
      return [initial, input.indeterminate, input.hasAttribute("indeterminate")];
    },
  },
  {
    group: "forms",
    name: "form-owner-follows-form-attribute",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      form.id = "owner";
      const input = document.createElement("input");
      input.setAttribute("form", "owner");
      document.body.append(form, input);
      return [input.form === form, form.elements.length];
    },
  },
  {
    group: "forms",
    name: "form-and-submitter-settings-reflect",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      const submit = document.createElement("button");
      form.action = "/form";
      form.method = "post";
      submit.formAction = "/submit";
      submit.formMethod = "dialog";
      return [form.action, form.method, submit.formAction, submit.formMethod];
    },
  },
  {
    group: "forms",
    name: "form-data-uses-successful-controls",
    run(window) {
      const { document, FormData } = window;
      reset(document);
      const form = document.createElement("form");
      form.innerHTML =
        '<input name="text" value="one"><input type="checkbox" name="check" checked><input name="off" disabled value="no">';
      document.body.appendChild(form);
      return Array.from(new FormData(form).entries());
    },
  },
  {
    group: "forms",
    name: "required-control-blocks-submit",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      const input = document.createElement("input");
      input.required = true;
      const submit = document.createElement("button");
      form.append(input, submit);
      document.body.appendChild(form);
      let submitted = 0;
      form.addEventListener("submit", () => submitted++);
      submit.click();
      return submitted;
    },
  },
  {
    group: "forms",
    name: "submitter-no-validate-bypasses-constraints",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      const input = document.createElement("input");
      input.required = true;
      const submit = document.createElement("input");
      submit.type = "submit";
      submit.formNoValidate = true;
      form.append(input, submit);
      document.body.appendChild(form);
      let submitter = null;
      form.addEventListener("submit", (event) => {
        submitter = event.submitter;
      });
      submit.click();
      return submitter === submit;
    },
  },
  {
    group: "forms",
    name: "reset-control-restores-default-value",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      const input = document.createElement("input");
      input.defaultValue = "markup";
      const resetControl = document.createElement("button");
      resetControl.type = "reset";
      form.append(input, resetControl);
      document.body.appendChild(form);
      input.value = "changed";
      resetControl.click();
      return input.value;
    },
  },
  {
    group: "forms",
    name: "radio-click-is-exclusive-by-form-owner",
    run(window) {
      const { document } = window;
      reset(document);
      const form = document.createElement("form");
      const first = document.createElement("input");
      const second = document.createElement("input");
      first.type = second.type = "radio";
      first.name = second.name = "choice";
      form.append(first, second);
      document.body.appendChild(form);
      first.click();
      second.click();
      return [first.checked, second.checked];
    },
  },
  {
    group: "forms",
    name: "label-click-activates-nested-checkbox",
    run(window) {
      const { document } = window;
      reset(document);
      const label = document.createElement("label");
      const input = document.createElement("input");
      input.type = "checkbox";
      label.appendChild(input);
      document.body.appendChild(label);
      label.click();
      return input.checked;
    },
  },
  {
    group: "forms",
    name: "disabled-fieldset-excludes-validation-and-data",
    run(window) {
      const { document, FormData } = window;
      reset(document);
      const form = document.createElement("form");
      const fieldset = document.createElement("fieldset");
      fieldset.disabled = true;
      const input = document.createElement("input");
      input.name = "hidden";
      input.value = "value";
      input.required = true;
      fieldset.appendChild(input);
      form.appendChild(fieldset);
      document.body.appendChild(form);
      return [form.checkValidity(), Array.from(new FormData(form).entries())];
    },
  },
  {
    group: "events",
    name: "cancel-bubble-is-the-stop-propagation-flag",
    run(window) {
      const { document, Event } = window;
      reset(document);
      const target = document.createElement("div");
      const before = [];
      target.addEventListener("ping", (event) => {
        before.push(event.cancelBubble);
        event.stopPropagation();
        before.push(event.cancelBubble);
      });
      target.dispatchEvent(new Event("ping"));
      // And the setter, which only ever sets the flag.
      const written = new Event("pong");
      written.cancelBubble = false;
      const unchanged = written.cancelBubble;
      written.cancelBubble = true;
      return [...before, unchanged, written.cancelBubble];
    },
  },
  {
    group: "tree",
    name: "nodes-stringify-as-their-interface",
    run(window) {
      const { document } = window;
      reset(document);
      const tag = (value) => Object.prototype.toString.call(value);
      return [
        tag(document),
        tag(document.createElement("div")),
        tag(document.createElement("unknown-tag")),
        tag(document.createTextNode("t")),
        tag(document.createComment("c")),
        tag(document.createDocumentFragment()),
        tag(document.createElement("div").style),
        tag(document.createElement("div").classList),
      ];
    },
  },
  {
    group: "parsing",
    name: "svg-elements-get-their-own-interface",
    run(window) {
      const { document } = window;
      reset(document);
      document.body.innerHTML = "<svg><g><circle/></g><text><tspan/></text><linearGradient><stop/></linearGradient></svg>";
      const name = (selector) => document.querySelector(selector).constructor.name;
      const svg = document.body.firstElementChild;
      const g = document.querySelector("g");
      return [
        name("svg"), name("g"), name("circle"), name("text"), name("tspan"),
        name("linearGradient"), name("stop"),
        g instanceof window.SVGGraphicsElement,
        g instanceof window.SVGElement,
        // Case-sensitive, and an unknown name keeps the base interface.
        document.createElementNS("http://www.w3.org/2000/svg", "CIRCLE").constructor.name,
        svg.namespaceURI,
      ];
    },
  },
  {
    group: "components",
    name: "a-customized-built-in-upgrades-its-built-in",
    run(window) {
      const { document } = window;
      reset(document);
      const name = unique("built-in");
      const log = [];
      class Customized extends window.HTMLButtonElement {
        constructor() { super(); log.push("constructed"); }
        connectedCallback() { log.push("connected"); }
      }
      window.customElements.define(name, Customized, { extends: "button" });
      const created = document.createElement("button", { is: name });
      document.body.appendChild(created);
      const host = document.createElement("div");
      host.innerHTML = `<button is="${name}"></button>`;
      document.body.appendChild(host);
      // The attribute set after creation customizes nothing.
      const late = document.createElement("button");
      late.setAttribute("is", name);
      document.body.appendChild(late);
      return [
        created instanceof Customized,
        // Created with the option, so there is no attribute — but the
        // serializer writes the is value all the same.
        created.getAttribute("is"),
        created.outerHTML,
        created.matches(":defined"),
        host.firstElementChild instanceof Customized,
        late instanceof Customized,
        document.createElement("div", { is: name }) instanceof Customized,
        new Customized().localName,
        log,
        errorName(() => window.customElements.define(unique("other"), class extends window.HTMLElement {}, { extends: "not-a-real-tag" })),
      ];
    },
  },
  {
    group: "cascade",
    name: "custom-properties-substitute-into-computed-values",
    run(window) {
      const { document } = window;
      reset(document);
      const style = document.createElement("style");
      style.textContent = `
        .root { --space: 8px; --alias: var(--space); --loop: var(--other); --other: var(--loop); }
        .child { padding-left: var(--space); margin-left: var(--alias);
                 text-align: var(--nothing); word-spacing: var(--missing, 4px);
                 letter-spacing: var(--loop, 3px); }
      `;
      document.head.appendChild(style);
      const root = document.createElement("div");
      root.className = "root";
      const child = document.createElement("span");
      child.className = "child";
      root.appendChild(child);
      document.body.appendChild(root);
      const computed = window.getComputedStyle(child);
      const read = (name) => computed.getPropertyValue(name);
      const result = [
        read("padding-left"),
        // Through one custom property to another.
        read("margin-left"),
        // No value and no fallback: invalid at computed-value time, so the
        // initial value stands.
        read("text-align"),
        read("word-spacing"),
        // A cycle is no value either, so the fallback is used.
        read("letter-spacing"),
        read("--space"),
        read("--alias"),
        read("--loop"),
      ];
      style.remove();
      return result;
    },
  },
  {
    group: "cascade",
    name: "a-value-a-property-cannot-take-is-dropped",
    run(window) {
      const { document, CSS } = window;
      reset(document);
      const element = document.createElement("div");
      const pairs = [
        // A bare number needs a unit, except where the property takes one.
        ["width", "23"], ["mask-position", "23"], ["margin", "5px 23"],
        ["line-height", "1.5"], ["z-index", "3"], ["padding", "0"],
        // And a property that takes a number does not always take a length.
        ["opacity", "2px"], ["border-image-slice", "2px"], ["stroke-width", "2px"],
        // Unknown units, and the functions that are nobody's business here.
        ["width", "5foo"], ["width", "calc(100% - 23px)"], ["color", "var(--x)"],
      ];
      return pairs.map(([property, value]) => {
        element.style.setProperty(property, value);
        const kept = element.style.getPropertyValue(property) !== "";
        element.style.removeProperty(property);
        return [property, value, CSS.supports(property, value), kept];
      });
    },
  },
  {
    group: "cascade",
    name: "shorthands-expand-into-longhands",
    run(window) {
      const { document } = window;
      reset(document);
      const style = document.createElement("style");
      style.textContent = `
        .box {
          margin: 1px 2px 3px 4px;
          padding: 5px;
          border: 2px solid blue;
          border-left-width: 9px;
          border-radius: 10px;
          outline: 3px dashed;
          flex: 2 3 40px;
          gap: 6px 7px;
          overflow: hidden auto;
          font: italic bold 12px/1.5 serif;
          grid-row: 1 / 3;
          list-style: square inside;
          text-decoration: underline wavy;
        }
      `;
      document.head.appendChild(style);
      const box = document.createElement("div");
      box.className = "box";
      document.body.appendChild(box);
      const computed = window.getComputedStyle(box);
      const read = (name) => computed.getPropertyValue(name);
      const result = [
        [read("margin-top"), read("margin-right"), read("margin-bottom"), read("margin-left")],
        [read("padding-top"), read("padding-left")],
        // A longhand written after the shorthand wins, which is the whole
        // reason a shorthand has to become longhands in the cascade.
        [read("border-top-width"), read("border-left-width"), read("border-top-style")],
        [read("border-top-left-radius"), read("border-bottom-right-radius")],
        [read("outline-width"), read("outline-style")],
        [read("flex-grow"), read("flex-shrink"), read("flex-basis")],
        [read("row-gap"), read("column-gap")],
        [read("overflow-x"), read("overflow-y")],
        [read("font-size"), read("line-height"), read("font-style"), read("font-weight"), read("font-family")],
        [read("grid-row-start"), read("grid-row-end")],
        [read("list-style-type"), read("list-style-position")],
        [read("text-decoration-line"), read("text-decoration-style")],
      ];
      style.remove();
      return result;
    },
  },
  {
    group: "tree",
    name: "named-access-on-the-window",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      element.id = "matrixNamed";
      document.body.appendChild(element);
      const form = document.createElement("form");
      form.name = "matrixNamedForm";
      document.body.appendChild(form);
      const plain = document.createElement("div");
      plain.setAttribute("name", "matrixNamedPlain");
      document.body.appendChild(plain);
      const loose = document.createElement("div");
      loose.id = "matrixNamedLoose";
      const result = [
        window.matrixNamed === element,
        "matrixNamed" in window,
        // Behind the window, not on it: a real window property is never
        // shadowed by an id, and this is how that works.
        Object.hasOwn(window, "matrixNamed"),
        typeof window.location,
        // `name` counts for a form, and not for a div.
        window.matrixNamedForm === form,
        window.matrixNamedPlain === undefined,
        // Out of the document is out of the window.
        window.matrixNamedLoose === undefined,
      ];
      element.id = "matrixRenamed";
      result.push(window.matrixNamed === undefined, window.matrixRenamed === element);
      element.remove();
      result.push(window.matrixRenamed === undefined);
      return result;
    },
  },
  {
    group: "cascade",
    name: "an-inline-declaration-holds-its-longhands",
    run(window) {
      const { document } = window;
      reset(document);
      const style = document.createElement("div").style;
      const once = (name, value) => {
        style.setProperty(name, String(value));
        const read = style.getPropertyValue(name);
        style.removeProperty(name);
        return read;
      };
      // A bare zero is a length almost everywhere, and serializes with its unit.
      const zeros = [
        once("width", 0), once("margin", 0), once("border-top-width", 0), once("top", 0),
        once("letter-spacing", 0), once("font-size", 0), once("flex-basis", 0), once("cx", 0),
        // …except where zero really is a number.
        once("line-height", 0), once("z-index", 0), once("opacity", 0), once("stroke-width", 0),
        once("tab-size", 0), once("scale", 0),
        once("width", "0.0"), once("width", "0px"),
      ];
      // A shorthand answers for the properties it covers, in both directions.
      const border = document.createElement("div").style;
      border.border = "1px solid red";
      const margin = document.createElement("div").style;
      margin.margin = "1px 2px";
      const font = document.createElement("div").style;
      font.font = "italic 12px/1.5 serif";
      return [
        zeros,
        [border.borderTopWidth, border.borderBottom, border.border, border.borderWidth, border.borderColor, border.cssText],
        [margin.marginTop, margin.marginRight, margin.marginBottom, margin.margin, margin.cssText],
        [font.fontSize, font.lineHeight, font.fontFamily, font.fontStyle],
      ];
    },
  },
  {
    group: "forms",
    name: "a-controls-value-is-filtered-not-rewritten",
    run(window) {
      const { document } = window;
      reset(document);
      const number = (value) => {
        const input = document.createElement("input");
        input.type = "number";
        input.value = value;
        return input.value;
      };
      const range = (attributes, value) => {
        const input = document.createElement("input");
        input.type = "range";
        for (const [name, setting] of Object.entries(attributes)) input.setAttribute(name, setting);
        input.value = value;
        return input.value;
      };
      const select = document.createElement("select");
      const single = select.type;
      select.multiple = true;
      const size = document.createElement("input");
      return [
        // A number keeps the string it was given, or loses it entirely.
        [number("1.00"), number(" 1.5 "), number("abc"), number("1e3"), number("-0"), number("007"), number("")],
        // A range is the exception: clamped, snapped, and defaulted to the middle.
        [
          range({ min: "0", max: "100" }, "200"),
          range({ min: "10", max: "100" }, "5"),
          range({ min: "0", max: "100" }, "42.5"),
          range({ min: "0", max: "100" }, "abc"),
          range({}, "200"),
          range({ min: "0", max: "10", step: "2" }, "5"),
          range({ min: "0", max: "10", step: "any" }, "4.7"),
        ],
        [single, select.type],
        // `size` is limited to positive numbers, and says so rather than clamping.
        [size.size, errorName(() => { size.size = 0; }), size.size, errorName(() => { size.size = 4294967296; })],
      ];
    },
  },
  {
    group: "tree",
    name: "interface-members-can-be-redefined",
    run(window) {
      const { document } = window;
      reset(document);
      // What a test framework does to observe a property: redefine it. A
      // descriptor that forgot `configurable` answers "Cannot redefine".
      const members = [
        [window.HTMLInputElement, "checked"], [window.HTMLInputElement, "value"],
        [window.Element, "id"], [window.Node, "textContent"], [window.Element, "innerHTML"],
        [window.Element, "classList"], [window.HTMLSelectElement, "options"],
        [window.Element, "attributes"], [window.HTMLElement, "attachInternals"],
      ];
      const notConfigurable = members
        .filter(([Class, name]) => {
          const found = Object.getOwnPropertyDescriptor(Class.prototype, name);
          return found !== undefined && !found.configurable;
        })
        .map(([Class, name]) => `${Class.name}.${name}`);
      const input = document.createElement("input");
      const original = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "checked");
      const seen = [];
      Object.defineProperty(input, "checked", {
        configurable: true,
        get: original.get,
        set(value) { seen.push(value); original.set.call(this, value); },
      });
      input.checked = true;
      return [notConfigurable, seen, input.checked];
    },
  },
  {
    group: "events",
    name: "an-attribute-handler-is-compiled-and-a-throw-is-reported",
    run(window) {
      const { document } = window;
      reset(document);
      window.matrixHits = [];
      const button = document.createElement("button");
      document.body.appendChild(button);
      button.setAttribute("onclick", "matrixHits.push(['attribute', this.tagName, event.type])");
      const compiled = [typeof button.onclick, button.onclick === button.onclick];
      button.click();
      // Assigning the property does not write the attribute…
      const own = function () { window.matrixHits.push(["property"]); };
      button.onclick = own;
      const kept = [button.getAttribute("onclick") === null, button.onclick === own];
      button.click();
      // …and changing the attribute takes the slot back.
      button.setAttribute("onclick", "matrixHits.push(['again'])");
      const retaken = [typeof button.onclick, button.onclick === own];
      button.click();
      // Removing it empties the slot.
      const other = document.createElement("div");
      other.setAttribute("onclick", "matrixHits.push(['never'])");
      const hadOne = typeof other.onclick;
      other.removeAttribute("onclick");
      // Markup, not just setAttribute, and the element's own scope.
      const host = document.createElement("div");
      host.innerHTML = "<input oninput=\"matrixHits.push(['scope', value])\">";
      document.body.appendChild(host);
      host.firstElementChild.value = "typed";
      host.firstElementChild.dispatchEvent(new window.Event("input"));
      // A listener that throws does not reach the dispatcher.
      const thrower = document.createElement("div");
      thrower.addEventListener("boom", () => { window.matrixHits.push(["before"]); throw new Error("reported"); });
      thrower.addEventListener("boom", () => window.matrixHits.push(["after"]));
      let propagated = null;
      let returned = null;
      const quiet = (event) => event.preventDefault();
      window.addEventListener("error", quiet);
      try {
        returned = thrower.dispatchEvent(new window.Event("boom", { cancelable: true }));
      } catch (error) {
        propagated = "threw";
      }
      window.removeEventListener("error", quiet);
      const hits = window.matrixHits.slice();
      delete window.matrixHits;
      return [compiled, kept, retaken, [hadOne, other.onclick], hits, [propagated, returned]];
    },
  },
  {
    group: "cascade",
    name: "a-colour-resolves-when-it-is-computed",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      document.body.appendChild(element);
      const both = (property, value) => {
        element.style.setProperty(property, value);
        const specified = element.style.getPropertyValue(property);
        const computed = window.getComputedStyle(element).getPropertyValue(property);
        element.style.removeProperty(property);
        return [specified, computed];
      };
      const values = [
        // A name survives the declaration and resolves in the computed value.
        both("color", "red"), both("color", "RED"), both("color", "rebeccapurple"),
        // A hex or a legacy function is canonical in both.
        both("color", "#fff"), both("color", "rgb(1, 2, 3)"), both("color", "rgb(1 2 3)"),
        both("color", "rgba(1, 2, 3, 0.5)"), both("color", "rgb(1 2 3 / 50%)"),
        both("color", "hsl(0, 100%, 50%)"),
        // The two keywords only a computed value can answer.
        both("color", "transparent"), both("color", "currentcolor"),
        // A modern colour keeps the space it was written in.
        both("color", "oklch(0.5 0.1 200)"),
        // And the same rules on the other colour properties.
        both("background-color", "red"), both("border-top-color", "red"),
        both("outline-color", "red"), both("fill", "red"), both("caret-color", "red"),
      ];
      // `currentcolor` elsewhere is whatever `color` computed to, inherited
      // through the tree like any other colour.
      const parent = document.createElement("div");
      const child = document.createElement("span");
      parent.appendChild(child);
      document.body.appendChild(parent);
      parent.style.color = "rgb(10, 20, 30)";
      child.style.borderTopColor = "currentcolor";
      const computed = window.getComputedStyle(child);
      return [values, [computed.color, computed.borderTopColor]];
    },
  },
  {
    group: "cascade",
    name: "a-value-is-checked-against-the-property",
    run(window) {
      const { document, CSS } = window;
      reset(document);
      const style = document.createElement("div").style;
      const once = (property, value) => {
        style.setProperty(property, value);
        const kept = style.getPropertyValue(property);
        style.removeProperty(property);
        return [CSS.supports(property, value), kept];
      };
      return [
        // A bare zero is a length that needs no unit — and not a time or an
        // angle, which is why a browser drops these two. (`box-shadow` compares
        // validity only: a browser also *reorders* the components, which this
        // DOM does not.)
        once("box-shadow", "0 0 2px red")[0], once("transition-duration", "0"), once("rotate", "0"),
        once("transition-duration", "0s"), once("rotate", "0deg"),
        // Negatives, per property rather than per value.
        once("width", "-5px"), once("margin", "-5px"), once("letter-spacing", "-5px"),
        once("padding", "-5px"), once("z-index", "-1"), once("flex-grow", "-1"),
        // A unit from the wrong family, and one from no family at all.
        once("width", "2s"), once("transition-duration", "2px"), once("width", "5foo"),
        once("grid-template-columns", "1fr"), once("width", "1fr"),
        // What a zero serializes as is the property's own answer.
        once("aspect-ratio", "0"), once("background-position", "0"), once("line-height", "0"),
        once("width", "0"), once("stroke-width", "0"), once("cx", "0"),
      ];
    },
  },
  {
    group: "cascade",
    name: "colour-functions-are-canonical",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      document.body.appendChild(element);
      const both = (value) => {
        element.style.setProperty("color", value);
        const specified = element.style.getPropertyValue("color");
        const computed = window.getComputedStyle(element).color;
        element.style.removeProperty("color");
        return [specified, computed];
      };
      return [
        // Hex and the legacy functions are `rgb()` in the declaration already,
        // clamped, rounded, and with the shortest alpha that names the same byte.
        both("#ff000080"), both("#f008"), both("rgb(300 0 0)"), both("rgb(-5 0 0)"),
        both("rgb(50% 0% 0%)"), both("rgb(1.5 2.4 3.6)"), both("rgba(255 0 0 / 30%)"),
        both("hsl(0 100% 50% / 30%)"), both("hsl(400 150% 50%)"),
        // `hwb()` too, whitened and blackened.
        both("hwb(0 0% 0%)"), both("hwb(120 10% 20%)"), both("hwb(120 10% 20% / 0.5)"),
        both("hwb(0 0% 0% / 50%)"),
        // These keep their space and lose their sugar.
        both("lab(50% 40 30)"), both("lab(50% 40 30 / 0.5)"), both("lch(50 40 30deg)"),
        both("oklch(50% 0.1 200)"), both("oklab(0.5 0.1 0.1)"), both("color(srgb 1 0 0)"),
        // `color-mix` drops the space it can assume, and an sRGB mix resolves.
        both("color-mix(in oklab, red, blue)")[0],
        both("color-mix(in srgb, red, blue)"), both("color-mix(in srgb, red 30%, blue)"),
      ];
    },
  },
  {
    group: "cascade",
    name: "lengths-are-absolute-when-they-can-be",
    run(window) {
      const { document } = window;
      reset(document);
      const sheet = new window.CSSStyleSheet();
      document.adoptedStyleSheets = [sheet];
      const read = (css, markup, selector, properties) => {
        sheet.replaceSync(css);
        const host = document.createElement("div");
        host.innerHTML = markup;
        document.body.appendChild(host);
        const target = selector ? host.querySelector(selector) : host.firstElementChild;
        const computed = window.getComputedStyle(target);
        const answer = properties.map((property) => computed.getPropertyValue(property));
        host.remove();
        return answer;
      };
      const result = [
        // The root size, and the three ways of being relative to it.
        read("", "<p>x</p>", null, ["font-size", "line-height"]),
        read(".a { font-size: 2em }", "<p class=a>x</p>", null, ["font-size"]),
        read(".a { font-size: 2em } .b { font-size: 0.5em }", "<div class=a><span class=b>x</span></div>", ".b", ["font-size"]),
        read(".a { font-size: 50% }", "<p class=a>x</p>", null, ["font-size"]),
        read(".a { font-size: 2rem }", "<p class=a>x</p>", null, ["font-size"]),
        // The keyword sizes, including the two relative ones.
        ["xx-small", "small", "medium", "large", "xx-large", "smaller", "larger"]
          .map((keyword) => read(`.a { font-size: ${keyword} }`, "<p class=a>x</p>", null, ["font-size"])[0]),
        // The user-agent sheet's headings, which are written in `em`.
        ["h1", "h3", "h6"].map((tag) => read("", `<${tag}>x</${tag}>`, null, ["font-size"])[0]),
        // `line-height` takes a number, a percentage, a length — and `normal`,
        // which resolves against a font this DOM is not measuring.
        [
          read(".a { font-size: 10px; line-height: 2 }", "<p class=a>x</p>", null, ["line-height"])[0],
          read(".a { font-size: 10px; line-height: 150% }", "<p class=a>x</p>", null, ["line-height"])[0],
          read(".a { font-size: 10px; line-height: 2em }", "<p class=a>x</p>", null, ["line-height"])[0],
          read(".a { font-size: 10px; line-height: normal }", "<p class=a>x</p>", null, ["line-height"])[0],
        ],
        // Other lengths: `em` and `rem` resolve, a percentage of the containing
        // block does not, and a border with no style has no width.
        read(".a { font-size: 10px; letter-spacing: 2em; word-spacing: 1em; text-indent: 50%; border-top-width: 0.5em }",
          "<p class=a>x</p>", null, ["letter-spacing", "word-spacing", "text-indent", "border-top-width"]),
        // An inherited font size is what a child's own relative value resolves against.
        read(".a { font-size: 20px } .b { line-height: 1.5 }", "<div class=a><span class=b>x</span></div>", ".b", ["font-size", "line-height"]),
      ];
      document.adoptedStyleSheets = [];
      return result;
    },
  },
  {
    group: "cascade",
    name: "the-layered-shorthands-expand-too",
    run(window) {
      const { document } = window;
      reset(document);
      const style = document.createElement("div").style;
      const read = (declaration, longhands) => {
        style.cssText = declaration;
        const answer = longhands.map((name) => style.getPropertyValue(name));
        style.cssText = "";
        return answer;
      };
      return [
        // A time is the duration the first time and the delay the second, and a
        // layer that names neither takes the defaults.
        read("transition: opacity 2s", ["transition-property", "transition-duration", "transition-timing-function", "transition-delay", "transition-behavior"]),
        read("transition: opacity 2s ease-in 1s, color 3s", ["transition-property", "transition-duration", "transition-timing-function", "transition-delay"]),
        read("animation: spin 2s linear 1s infinite alternate both running", ["animation-name", "animation-duration", "animation-timing-function", "animation-delay", "animation-iteration-count", "animation-direction", "animation-fill-mode", "animation-play-state"]),
        read("animation: 3s spin", ["animation-name", "animation-duration", "animation-timing-function", "animation-delay", "animation-iteration-count"]),
        // Rows before the slash, columns after — with the area strings
        // interleaved in the rows, and quoted as a browser quotes them.
        read("grid-template: 1fr 2fr / 100px 200px", ["grid-template-rows", "grid-template-columns", "grid-template-areas"]),
        read("grid-template: 'a b' 50px 'c d' 1fr / 100px auto", ["grid-template-rows", "grid-template-columns", "grid-template-areas"]),
        // `mask`, read by what each token can be, with the size after the slash.
        read("mask: url(m.svg) center / cover no-repeat", ["mask-image", "mask-position", "mask-size", "mask-repeat", "mask-origin", "mask-clip", "mask-composite", "mask-mode"]),
        read("offset: path('M 0 0 L 10 10') 50% 90deg / auto", ["offset-path", "offset-distance", "offset-rotate", "offset-anchor", "offset-position"]),
      ];
    },
  },
  {
    group: "cascade",
    name: "a-computed-shorthand-comes-from-its-longhands",
    run(window) {
      const { document } = window;
      reset(document);
      const element = document.createElement("div");
      element.style.cssText = "border: 1px solid red; margin: 1px 2px; padding: 3px; border-radius: 4px";
      document.body.appendChild(element);
      const computed = window.getComputedStyle(element);
      const read = (name) => computed.getPropertyValue(name);
      const plain = document.createElement("div");
      document.body.appendChild(plain);
      const bare = window.getComputedStyle(plain);
      return [
        // A computed style holds no shorthands, so each of these is serialized
        // back out of the parts — which is how `border` reports the colour it
        // computed rather than the name that was written.
        [read("border"), read("border-top"), read("border-width"), read("border-style"), read("border-color")],
        [read("margin"), read("padding"), read("border-radius")],
        // And with nothing declared, the initial values collapse the same way.
        [bare.margin, bare.padding, bare.borderWidth, bare.gap, bare.overflow],
      ];
    },
  },
  {
    group: "cascade",
    name: "a-declaration-block-holds-longhands",
    run(window) {
      const { document } = window;
      reset(document);
      const style = document.createElement("div").style;
      const after = (steps) => {
        style.cssText = "";
        const seen = [];
        for (const step of steps) {
          step(style);
          seen.push([style.cssText, style.length]);
        }
        style.cssText = "";
        return seen;
      };
      const items = (declaration) => {
        style.cssText = declaration;
        const names = Array.from({ length: style.length }, (_, index) => style.item(index));
        style.cssText = "";
        return names;
      };
      const text = (declaration) => {
        style.cssText = declaration;
        const serialized = style.cssText;
        style.cssText = "";
        return serialized;
      };
      return [
        // A shorthand is stored as its longhands, and serialized back out of
        // them — so overriding one part drops to the sub-shorthands.
        after([(s) => { s.border = "1px solid red"; }, (s) => { s.borderTopWidth = "9px"; }]),
        // A family completed one longhand at a time collapses once it is whole.
        after([(s) => { s.marginTop = "1px"; }, (s) => { s.marginRight = "2px"; }, (s) => { s.marginBottom = "1px"; }, (s) => { s.marginLeft = "2px"; }]),
        after([(s) => { s.margin = "1px"; }, (s) => { s.removeProperty("margin-top"); }]),
        items("border: 1px solid red"),
        items("background: red url(x.png) no-repeat"),
        // Each family prints in its own order, which is not the order it expands
        // in: `outline` prints colour, style, width where `border` prints width,
        // style, colour.
        [
          text("color: red; margin: 1px !important; padding: 2px"),
          text("box-shadow: inset 0 0 2px red"),
          text("background: red url(x.png) no-repeat fixed left top / cover content-box padding-box"),
          text("border-image: url(b.png) 30 fill / 10px / 2px round"),
          text("outline: 2px dashed red"),
          text("list-style: square inside url(b.png)"),
          text("text-decoration: underline wavy red 2px"),
          text("font: italic small-caps bold 12px/1.5 serif"),
          text("animation: spin 2s linear 1s infinite alternate both running"),
          text("transition: opacity 2s 1s"),
          text("grid-template: 'a b' 50px 'c d' 1fr / 100px auto"),
          text("mask: url(m.svg) center / cover no-repeat"),
          text("offset: path('M 0 0 L 10 10') 50% 90deg / auto"),
        ],
      ];
    },
  },
  {
    group: "tree",
    name: "every-html-name-has-its-interface",
    run(window) {
      const { document } = window;
      reset(document);
      const name = (tag) => document.createElement(tag).constructor.name;
      return [
        // The ones a framework is most likely to branch on.
        [name("script"), name("img"), name("link"), name("meta"), name("iframe"), name("video"), name("audio")],
        [name("h1"), name("h6"), name("p"), name("span"), name("br"), name("ul"), name("li")],
        // Two names sharing one interface, and two sharing another.
        [name("ins"), name("del"), name("blockquote"), name("q")],
        // A media element is a media element before it is an HTML element.
        [
          document.createElement("video") instanceof window.HTMLMediaElement,
          document.createElement("video") instanceof window.HTMLElement,
        ],
        // A name the language does not have is unknown; a name it could still be
        // given is not.
        [name("nonsense"), name("my-thing")],
        // And the interface says its own name.
        Object.prototype.toString.call(document.createElement("script")),
      ];
    },
  },
  {
    group: "tree",
    name: "a-template-content-lives-in-its-own-document",
    run(window) {
      const { document } = window;
      reset(document);
      const template = document.createElement("template");
      const child = template.content.appendChild(document.createElement("span"));
      const inert = document.implementation.createHTMLDocument("");
      const before = template.content.ownerDocument;
      const result = [
        // The content belongs to the template contents owner, not to the
        // template's own document.
        before === document,
        before === template.ownerDocument,
        // Adopting the content itself is allowed and takes its children with it.
        inert.adoptNode(template.content) === template.content,
        template.content.ownerDocument === inert,
        child.ownerDocument === inert,
        // A shadow root is a fragment with a host, and cannot be adopted away.
        errorName(() => {
          const host = document.createElement("div");
          inert.adoptNode(host.attachShadow({ mode: "closed" }));
        }),
      ];
      // Adopting the *element* runs the template adopting steps: the content
      // follows into the new document's own contents owner.
      const source = document.implementation.createHTMLDocument("");
      const moving = source.createElement("template");
      const inner = moving.content.appendChild(source.createElement("i"));
      const contentDocument = moving.content.ownerDocument;
      const target = document.implementation.createHTMLDocument("");
      target.adoptNode(moving);
      result.push(
        moving.ownerDocument === target,
        moving.content.ownerDocument === contentDocument,
        inner.ownerDocument === moving.content.ownerDocument,
      );
      return result;
    },
  },
];

// Async, because several of these behaviours are: a `slotchange` is delivered at
// the microtask checkpoint, `whenDefined` is a promise, and a mutation record
// arrives after the mutation. A case that needs none of that just returns.
export async function runCases(window) {
  const results = [];
  for (const { expectedEsdev, group, limit, name, run } of cases) {
    const entry = { expectedEsdev: expectedEsdev ?? null, group, limit: limit ?? null, name };
    try {
      entry.result = await run(window);
    } catch (error) {
      entry.error = error?.name ?? "Error";
    }
    results.push(entry);
  }
  return results;
}

// One turn of the microtask queue, for a case waiting on a reaction that is
// delivered there.
export function settled() {
  return new Promise((resolve) => queueMicrotask(resolve));
}
