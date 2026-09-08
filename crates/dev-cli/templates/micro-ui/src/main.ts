import { define, html, mount, update } from "@opentf/micro-ui";

import "@opentf/micro-ui/styles.css";

const root = document.getElementById("app");
if (!root) {
  throw new Error("index.html has no #app for this module to render into");
}

define("x-counter", (el, props) => {
  let count = Number(props.start || 0);

  return () => html`
    <article class="ui-card">
      <p class="ui-eyebrow">Micro-UI component</p>
      <h1>${props.title || "A tiny web app"}</h1>
      <p>State stays local, and updates are explicit.</p>
      <button class="ui-btn ui-btn-primary" onclick=${() => { count++; update(el); }}>
        Clicked ${count} times
      </button>
    </article>
  `;
});

mount(root, "x-counter", { dev: true });
