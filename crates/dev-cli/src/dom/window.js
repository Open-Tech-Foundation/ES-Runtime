// The sole public entry point for esdev's test-only DOM. Importing it installs
// a new realm-local document; the test runner imports it before the test entry.
import { createEvents } from "runtime:dom/events";
import { createTree } from "runtime:dom/tree";
import { createParsing } from "runtime:dom/parse";
import { createSelectors } from "runtime:dom/select";
import { createCss } from "runtime:dom/css";
import { createElements } from "runtime:dom/elements";

const events = createEvents();
const tree = createTree(events);
const parse = createParsing(tree, (source, context) =>
  globalThis.__ops.dom_parse_fragment(source, context));
const selectors = createSelectors(tree);
const css = createCss(tree);
const elements = createElements(tree);

const document = new tree.Document();
const html = document.createElement("html");
const head = document.createElement("head");
const body = document.createElement("body");
html.append(head, body);
document.appendChild(html);

Object.defineProperties(document, {
  head: { get: () => head },
  body: { get: () => body },
});
parse.install();
selectors.install();
css.install();
const customElements = elements.install(document);

Object.assign(globalThis, events, tree, css, elements, { document, customElements });
globalThis.window = globalThis;
