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
