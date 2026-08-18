/**
 * The browser's entry — named by the `<script type="module">` in index.html.
 *
 * It renders the page you are looking at, and it is the whole application:
 * there is no framework here and nothing else running. Replace it with yours.
 */
import { LEDE, LINKS, editHint } from "./page.ts";

const root = document.getElementById("app");
if (!root) {
  throw new Error("index.html has no #app for this module to render into");
}

root.replaceChildren(
  element("h1", { textContent: "{{name}}" }),
  element("p", { className: "lede", textContent: LEDE }),
  element("p", { className: "edit", textContent: editHint("src/main.ts") }),
  element(
    "nav",
    { className: "links" },
    LINKS.map((link) => element("a", { href: link.href, textContent: link.label })),
  ),
);

/**
 * A small `createElement`, typed.
 *
 * `textContent` is a *property*, never `innerHTML`, so a string that came from
 * somewhere else is text and can never be markup.
 */
function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  properties: Partial<HTMLElementTagNameMap[K]> = {},
  children: Node[] = [],
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  Object.assign(node, properties);
  node.append(...children);
  return node;
}
