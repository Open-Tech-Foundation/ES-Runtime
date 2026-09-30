import { assert, assertEquals, test } from "runtime:test";
import react from "../src/index.js";

const hot = { hot: true, platform: "browser" };

test("refresh is confined to hot browser builds", () => {
  for (const ctx of [
    { hot: false, platform: "browser" },
    { hot: true, platform: "server" },
    { command: "test", platform: "browser" },
    {},
  ]) {
    assertEquals(react.transform.handler("export {};", "/app/a.jsx", ctx), null);
  }
});

test("the source filter covers JSX and TSX", () => {
  for (const id of ["/app/a.jsx", "/app/a.tsx"]) assert(react.transform.filter.id.test(id));
  for (const id of ["/app/a.js", "/app/a.ts", "/app/a.css"]) {
    assert(!react.transform.filter.id.test(id));
  }
});

test("each original line maps past the complete generated prefix", () => {
  const code = "export const a = 1;\nexport const b = 2;\n";
  const id = "/app/a.tsx";
  const result = react.transform.handler(code, id, hot);
  const map = JSON.parse(result.map);
  const offset = result.code.slice(0, result.code.indexOf(code)).split("\n").length - 1;
  assertEquals(map.mappings.split(";").slice(0, offset), Array(offset).fill(""));
  assertEquals(map.mappings.split(";").slice(offset), ["AAAA", "AACA", "AACA"]);
  assertEquals(map.sources, [id]);
  assertEquals(map.sourcesContent, [code]);
});

test("module identifiers are escaped as JavaScript strings", () => {
  const id = '/app/a"\\name\n.jsx';
  const result = react.transform.handler("export {};", id, hot);
  assert(result.code.includes(`${JSON.stringify(id)} + " " + name`));
});

test("an empty module still has a source map", () => {
  const result = react.transform.handler("", "/app/empty.jsx", hot);
  assertEquals(JSON.parse(result.map).sourcesContent, [""]);
});
