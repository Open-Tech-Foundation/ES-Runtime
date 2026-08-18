import { test, assertEquals } from "runtime:test";
import { Router } from "./router.ts";

const ok = () => new Response("ok");
const router = new Router([
  { method: "GET", path: "/items", handle: ok },
  { method: "POST", path: "/items", handle: ok },
  { method: "GET", path: "/items/:id", handle: ok },
  { method: "DELETE", path: "/items/:id", handle: ok },
]);

test("a path parameter reaches the handler", () => {
  const matched = router.match("GET", "/items/42");
  assertEquals(matched.kind, "found");
  assertEquals(matched.kind === "found" && matched.params.id, "42");
});

test("a path with no route is not found", () => {
  assertEquals(router.match("GET", "/nope").kind, "not-found");
  assertEquals(router.match("GET", "/items/42/extra").kind, "not-found");
});

test("a path that exists but not for this method is a 405, and says what works", () => {
  // The distinction a router keyed on "GET /items" throws away. A client told
  // only "no" learns nothing; one told `Allow` knows what to send next.
  const matched = router.match("PATCH", "/items/42");
  assertEquals(matched.kind, "method-not-allowed");
  assertEquals(matched.kind === "method-not-allowed" && matched.allowed.join(","), "DELETE,GET,HEAD,OPTIONS");
});

test("HEAD is answered by the GET route", () => {
  // Otherwise every route needs a second implementation that can disagree with
  // the first. The runtime drops the body.
  const matched = router.match("HEAD", "/items");
  assertEquals(matched.kind, "found");
});

test("a route with no GET does not claim to answer HEAD", () => {
  const posts = new Router([{ method: "POST", path: "/x", handle: ok }]);
  const matched = posts.match("GET", "/x");
  assertEquals(matched.kind === "method-not-allowed" && matched.allowed.join(","), "OPTIONS,POST");
});
