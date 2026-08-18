import { test, assert, assertEquals } from "runtime:test";
import { escape, head, pickMeta } from "./head.ts";

test("the deepest route that describes a title is the one that wins", () => {
  const meta = pickMeta(
    [
      { meta: () => ({ title: "layout" }), data: undefined },
      { meta: (d) => ({ title: (d as { name: string }).name }), data: { name: "post" } },
    ],
    { title: "fallback" },
  );
  assertEquals(meta.title, "post");
});

test("a route without a title is skipped, not treated as the answer", () => {
  const meta = pickMeta(
    [
      { meta: () => ({ title: "layout" }), data: undefined },
      { meta: undefined, data: { irrelevant: true } },
    ],
    { title: "fallback" },
  );
  assertEquals(meta.title, "layout");
});

test("a route with no loader still gets its static title", () => {
  // `data` is undefined for a route without a loader, which must not read as
  // "this route has nothing to say" — the index route is exactly this shape.
  const meta = pickMeta([{ meta: () => ({ title: "home" }), data: undefined }], {
    title: "fallback",
  });
  assertEquals(meta.title, "home");
});

test("nothing matching falls back rather than throwing", () => {
  assertEquals(pickMeta([], { title: "fallback" }).title, "fallback");
});

test("a route's meta becomes the tags that go in the head", () => {
  assertEquals(
    head({ title: "About", description: "Who made this." }),
    '<title>About</title><meta name="description" content="Who made this.">',
  );
});

test("a description is optional", () => {
  assertEquals(head({ title: "About" }), "<title>About</title>");
});

test("a title cannot close the tag it is inside", () => {
  // A page title is very often somebody else's string — a post's title, a
  // search term echoed back. This is the oldest injection there is.
  const injected = head({ title: "</title><script>alert(1)</script>" });
  assert(!injected.includes("<script>"), injected);
  assert(injected.includes("&lt;/title&gt;"), injected);
});

test("a description cannot break out of its attribute", () => {
  const injected = head({ title: "ok", description: '" onload="alert(1)' });
  assert(!injected.includes('" onload='), injected);
  assert(injected.includes("&quot;"), injected);
});

test("an ampersand is escaped first, so nothing is escaped twice", () => {
  // `&` last would turn the `&` of `&lt;` into `&amp;lt;` and render the
  // escaping visible on the page.
  assertEquals(escape("a & b < c"), "a &amp; b &lt; c");
  assertEquals(escape("&lt;"), "&amp;lt;");
});
