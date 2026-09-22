import { expect, test } from "runtime:test";
import { createCss } from "./css.js";
import { createSelectors } from "./select.js";
import { createSheets } from "./sheets.js";
import { createTree } from "./tree.js";
import { createColors } from "./colors.js";
import { color } from "./std-color.js";
import { CSS_VALUE_TABLE } from "./css-table.js";

// The records the stylesheet op produces, by hand: these cases are about the
// cascade, not about parsing CSS text.
const style = (selector, declarations) => [0, selector, declarations, []];
const group = (name, condition, rules) => [1, name, condition, rules];

function fixture({ sheets = {}, media = () => true } = {}) {
  const tree = createTree();
  const colors = createColors(color);
  const css = createCss({ ...tree, colors, valueTable: CSS_VALUE_TABLE });
  const selectors = createSelectors(tree);
  css.install();
  selectors.install();
  const parse = (text) => {
    if (!Object.hasOwn(sheets, text)) throw new SyntaxError(`Unexpected sheet: ${text}`);
    return sheets[text];
  };
  const module = createSheets({ tree, parse, selectors, css, mediaMatches: media, colors });
  const document = new tree.Document();
  const html = document.createElement("html");
  const head = document.createElement("head");
  const body = document.createElement("body");
  html.append(head, body);
  document.appendChild(html);
  Object.defineProperties(document, { head: { get: () => head }, body: { get: () => body } });
  module.install();
  return { body, document, head, module, tree };
}

function sheet(document, head, text) {
  const element = document.createElement("style");
  element.appendChild(document.createTextNode(text));
  head.appendChild(element);
  return element;
}

test("sorts by origin, importance, specificity and order", () => {
  const rules = [
    style("div", [["color", "one", false]]),
    style(".a", [["color", "two", false]]),
    style("#b", [["color", "three", false]]),
    style(".later", [["color", "four", false]]),
    style(".important", [["color", "five", true]]),
  ];
  const { document, head, body, module } = fixture({ sheets: { sheet: rules } });
  sheet(document, head, "sheet");
  const element = document.createElement("div");
  body.appendChild(element);

  expect(module.getComputedStyle(element).color).toBe("one");
  element.className = "a";
  expect(module.getComputedStyle(element).color).toBe("two");
  element.id = "b";
  expect(module.getComputedStyle(element).color).toBe("three");
  // Same specificity as `.a`, later in the sheet.
  element.className = "a later";
  expect(module.getComputedStyle(element).color).toBe("three");
  element.style.color = "inline";
  expect(module.getComputedStyle(element).color).toBe("inline");
  element.className = "a important";
  expect(module.getComputedStyle(element).color).toBe("five");
  element.style.setProperty("color", "inline", "important");
  expect(module.getComputedStyle(element).color).toBe("inline");
});

test("inherits what is inherited and nothing else", () => {
  const rules = [style(".parent", [["color", "purple", false], ["border-color", "red", false], ["--brand", "cyan", false]])];
  const { document, head, body, module } = fixture({ sheets: { sheet: rules } });
  sheet(document, head, "sheet");
  const parent = document.createElement("div");
  parent.className = "parent";
  const child = document.createElement("span");
  const grandchild = document.createElement("b");
  child.appendChild(grandchild);
  parent.appendChild(child);
  body.appendChild(parent);

  expect(module.getComputedStyle(child).color).toBe("rgb(128, 0, 128)");
  expect(module.getComputedStyle(grandchild).color).toBe("rgb(128, 0, 128)");
  expect(module.getComputedStyle(child).getPropertyValue("border-color")).toBe("");
  expect(module.getComputedStyle(child).getPropertyValue("--brand")).toBe("cyan");
  child.style.color = "orange";
  expect([module.getComputedStyle(child).color, module.getComputedStyle(grandchild).color]).toEqual(["rgb(255, 165, 0)", "rgb(255, 165, 0)"]);
});

test("falls back to the user-agent sheet and then the initial value", () => {
  const { document, body, module } = fixture();
  const div = document.createElement("div");
  const span = document.createElement("span");
  const strong = document.createElement("strong");
  const hidden = document.createElement("div");
  hidden.setAttribute("hidden", "");
  body.append(div, span, strong, hidden);

  expect(module.getComputedStyle(div).display).toBe("block");
  expect(module.getComputedStyle(span).display).toBe("inline");
  expect(module.getComputedStyle(hidden).display).toBe("none");
  expect(module.getComputedStyle(strong).fontWeight).toBe("700");
  expect(module.getComputedStyle(span).fontWeight).toBe("400");
  expect(module.getComputedStyle(span).visibility).toBe("visible");
  expect(module.getComputedStyle(span).color).toBe("rgb(0, 0, 0)");
  // Nothing outside the tree has a computed style, as in a browser.
  expect(module.getComputedStyle(document.createElement("div")).length).toBe(0);
});

test("applies a condition group only while its condition holds", () => {
  const rules = [
    group("media", "(min-width: 40em)", [style(".m", [["color", "wide", false]])]),
    group("media", "print", [style(".m", [["color", "printed", false]])]),
  ];
  let matches = true;
  const { document, head, body, module } = fixture({
    sheets: { sheet: rules },
    media: (condition) => matches && condition !== "print",
  });
  sheet(document, head, "sheet");
  const element = document.createElement("p");
  element.className = "m";
  body.appendChild(element);

  expect(module.getComputedStyle(element).color).toBe("wide");
  matches = false;
  expect(module.getComputedStyle(element).color).toBe("rgb(0, 0, 0)");
});

test("adopts a constructed sheet and drops it again", () => {
  const { document, body, module } = fixture({ sheets: { "adopted": [style(".c", [["color", "adopted", false]])] } });
  const element = document.createElement("div");
  element.className = "c";
  body.appendChild(element);
  const constructed = new module.CSSStyleSheet();
  constructed.replaceSync("adopted");

  expect(module.getComputedStyle(element).color).toBe("rgb(0, 0, 0)");
  document.adoptedStyleSheets = [constructed];
  expect([module.getComputedStyle(element).color, document.adoptedStyleSheets.length]).toEqual(["adopted", 1]);
  document.adoptedStyleSheets = [];
  expect(module.getComputedStyle(element).color).toBe("rgb(0, 0, 0)");
  expect(() => { document.adoptedStyleSheets = [{}]; }).toThrow("CSSStyleSheet");
});

test("ignores a rule it cannot match rather than failing the cascade", () => {
  const rules = [
    style("div::before", [["color", "pseudo", false]]),
    style("div:!!broken", [["color", "broken", false]]),
    style("div:hover", [["color", "hovered", false]]),
    style("div", [["color", "plain", false]]),
  ];
  const { document, head, body, module } = fixture({ sheets: { sheet: rules } });
  sheet(document, head, "sheet");
  const element = document.createElement("div");
  body.appendChild(element);

  expect(module.getComputedStyle(element).color).toBe("plain");
  expect(sheet(document, head, "sheet").sheet.cssRules.length).toBe(4);
});

test("a style element's sheet follows its text", () => {
  const { document, head, module } = fixture({
    sheets: { one: [style(".x", [["color", "one", false]])], two: [style(".x", [["color", "two", false]])] },
  });
  const element = sheet(document, head, "one");

  expect(element.sheet.cssRules[0].style.getPropertyValue("color")).toBe("one");
  expect(element.sheet.ownerNode).toBe(element);
  element.textContent = "two";
  expect(element.sheet.cssRules[0].style.getPropertyValue("color")).toBe("two");
  expect(document.styleSheets.length).toBe(1);
  expect(() => { element.sheet.cssRules[0].style.setProperty("color", "three"); }).toThrow("read-only");
  void module;
});
