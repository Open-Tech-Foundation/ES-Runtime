/**
 * The browser's entry — named by the `<script type="module">` in index.html,
 * which is what makes it a build input.
 *
 * There is no framework here, so the pattern worth showing is the one a
 * framework usually hides: **state in one place, one function that renders it,
 * and events that change the state rather than the DOM.**
 *
 * ```
 * event → update state → render()
 * ```
 *
 * That loop is what stops the two drifting apart. The alternative — a handler
 * that edits the DOM directly *and* keeps a variable in step — is where a
 * frameworkless app usually starts going wrong, because nothing makes the two
 * agree and nothing reports it when they stop.
 */
import { formatCount, nextId, type Item } from "./items.ts";
import styles from "./Counter.module.css";

const root = document.getElementById("app");
if (!root) {
  throw new Error("index.html has no #app for this module to render into");
}

/** The whole of the application's state. */
let items: Item[] = [];

/**
 * State to DOM, from scratch, every time.
 *
 * Rebuilding is not the fastest thing possible and it is the right default: it
 * is impossible for the page to disagree with `items`, because the page *is*
 * `items`. Reach for something finer only when a measurement says to.
 */
function render(): void {
  root!.replaceChildren(
    element("p", { className: styles.count!, textContent: formatCount(items.length) }),
    list(),
    element("button", {
      className: styles.add!,
      textContent: "Add one",
      onclick: () => {
        items = [...items, { id: nextId(items), label: `Item ${items.length + 1}` }];
        render();
      },
    }),
  );
}

function list(): HTMLElement {
  const ul = element("ul", { className: styles.list! });
  for (const item of items) {
    ul.append(
      element("li", { className: styles.item! }, [
        element("span", { textContent: item.label }),
        element("button", {
          className: styles.remove!,
          textContent: "Remove",
          // The id is captured rather than the index: an index goes stale the
          // moment anything before it is removed, which is the classic way a
          // list deletes the wrong row.
          onclick: () => {
            items = items.filter((other) => other.id !== item.id);
            render();
          },
        }),
      ]),
    );
  }
  return ul;
}

/**
 * A small `createElement`, typed.
 *
 * `document.createElement` plus assignment is already close to this; the value
 * here is that `textContent` is a *property*, never `innerHTML`, so a label
 * that came from somewhere else is text and can never be markup.
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

render();
