import { expect, test } from "runtime:test";
import { createTree } from "./tree.js";
import { createParsing } from "./parse.js";

function records(source) {
  if (source === "<em>new</em>") return [[1, -1, "em", [], ""], [3, 0, "", [], "new"]];
  throw new SyntaxError(`Unexpected fixture: ${source}`);
}

test("decodes flat records and serializes each HTML context correctly", () => {
  const tree = createTree();
  const document = new tree.Document();
  const parsing = createParsing(tree, records);
  const fragment = document.createDocumentFragment();
  parsing.decode([[1, -1, "main", [["title", "a&\"b"]], ""], [3, 0, "", [], "<hello>"], [8, 0, "", [], "note"]], fragment);
  expect(parsing.serialize(fragment)).toBe('<main title="a&amp;&quot;b">&lt;hello&gt;<!--note--></main>');
});

test("installs innerHTML and outerHTML through the supplied strict parser", () => {
  const tree = createTree();
  const parsing = createParsing(tree, records);
  parsing.install();
  const document = new tree.Document();
  const root = document.createElement("main");
  document.appendChild(root);
  root.innerHTML = "<em>new</em>";
  expect(root.outerHTML).toBe("<main><em>new</em></main>");
  root.firstChild.outerHTML = "<em>new</em>";
  expect(root.textContent).toBe("new");
});
