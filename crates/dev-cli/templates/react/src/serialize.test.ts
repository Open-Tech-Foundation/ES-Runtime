import { serialize } from "./serialize.ts";

test("the data reaches the browser as JSON", () => {
  assertEquals(
    serialize({ title: "Home" }),
    '<script>window.__DATA__={"title":"Home"}</script>',
  );
});

test("a string cannot close the script tag it is inside", () => {
  const escaped = serialize({ body: "</script><script>alert(1)</script>" });
  assert(!escaped.includes("</script><script>alert"), `not escaped: ${escaped}`);
  assert(escaped.includes("\\u003c/script"), `not escaped: ${escaped}`);
  // …and it is still the same data once the browser parses it.
  const json = escaped.slice(escaped.indexOf("=") + 1, escaped.lastIndexOf("</script>"));
  assertEquals(JSON.parse(json).body, "</script><script>alert(1)</script>");
});
