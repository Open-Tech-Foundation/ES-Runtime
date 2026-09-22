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

// `[doctype, records]`, the shape the document op returns.
function documentRecords(source) {
  if (source === "<!doctype html><p>one</p><style>a{}</style>") {
    return [true, [[1, -1, "p", [], ""], [3, 0, "", [], "one"], [1, -1, "style", [], ""], [3, 2, "", [], "a{}"]]];
  }
  if (source === "<html><head><title>T</title></head><body><main>y</main></body></html>") {
    return [false, [
      [1, -1, "html", [], ""], [1, 0, "head", [], ""], [1, 1, "title", [], ""], [3, 2, "", [], "T"],
      [1, 0, "body", [], ""], [1, 4, "main", [], ""], [3, 5, "", [], "y"],
    ]];
  }
  throw new SyntaxError(`Unexpected document fixture: ${source}`);
}

test("parses a bare document into a synthesized html, head and body", () => {
  const tree = createTree();
  const parsing = createParsing(tree, records, documentRecords);
  const parsed = new parsing.DOMParser().parseFromString("<!doctype html><p>one</p><style>a{}</style>", "text/html");

  expect(parsed).toBeInstanceOf(tree.Document);
  expect(parsed.doctype.name).toBe("html");
  expect(parsed.documentElement.localName).toBe("html");
  expect(parsed.firstElementChild).toBe(parsed.documentElement);
  expect(Array.from(parsed.head.children, (child) => child.localName)).toEqual(["style"]);
  expect(Array.from(parsed.body.children, (child) => child.localName)).toEqual(["p"]);
  expect(parsed.body.textContent).toBe("one");
});

test("keeps a document's own html, head and body when it supplies them", () => {
  const tree = createTree();
  const parsing = createParsing(tree, records, documentRecords);
  const parsed = new parsing.DOMParser().parseFromString("<html><head><title>T</title></head><body><main>y</main></body></html>", "text/html");

  expect(parsed.doctype).toBeNull();
  expect(parsed.title).toBe("T");
  expect(parsed.body.firstElementChild.localName).toBe("main");
  expect(parsed.documentElement.childElementCount).toBe(2);
});

test("refuses XML by name and an unsupported type by type", () => {
  const tree = createTree();
  const parsing = createParsing(tree, records, documentRecords);
  const parser = new parsing.DOMParser();

  expect(() => parser.parseFromString("<p/>", "text/xml")).toThrow("text/html only");
  expect(() => parser.parseFromString("<p></p>", "text/plain")).toThrow("not a supported");
  expect(() => createParsing(tree, records).DOMParser.prototype.parseFromString.call({}, "<p></p>", "text/html"))
    .toThrow("no document parser");
});
