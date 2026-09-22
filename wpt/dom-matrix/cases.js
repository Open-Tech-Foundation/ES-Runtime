// Layout-free DOM behavior shared by esdev, jsdom, and happy-dom. Every case
// returns JSON data rather than asserting so the runner can compare runtimes.

function reset(document) {
  document.body.replaceChildren();
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
];

export function runCases(window) {
  return cases.map(({ expectedEsdev, group, limit, name, run }) => {
    try {
      return {
        expectedEsdev: expectedEsdev ?? null,
        group,
        limit: limit ?? null,
        name,
        result: run(window),
      };
    } catch (error) {
      return {
        expectedEsdev: expectedEsdev ?? null,
        group,
        limit: limit ?? null,
        name,
        error: error?.name ?? "Error",
      };
    }
  });
}
