// WinterTC §2.4 — URL / URLSearchParams.

test("URL parses components", () => {
  const u = new URL("https://user:pass@example.com:8080/p/a?x=1#frag");
  assertEquals(u.protocol, "https:");
  assertEquals(u.username, "user");
  assertEquals(u.hostname, "example.com");
  assertEquals(u.port, "8080");
  assertEquals(u.pathname, "/p/a");
  assertEquals(u.search, "?x=1");
  assertEquals(u.hash, "#frag");
});

test("URL resolves relative references", () => {
  assertEquals(new URL("../b", "https://h.test/x/y/z").href, "https://h.test/x/b");
  assertEquals(new URL("//other.test/p", "https://h.test/").href, "https://other.test/p");
});

test("URL throws on invalid input", () => {
  assertThrows(() => new URL("not a url"), "TypeError");
});

test("URL default ports are dropped", () => {
  assertEquals(new URL("https://h.test:443/").port, "");
  assertEquals(new URL("http://h.test:80/").port, "");
});

test("URLSearchParams get/getAll/has", () => {
  const p = new URLSearchParams("a=1&a=2&b=3");
  assertEquals(p.get("a"), "1");
  assertEquals(p.getAll("a").join(","), "1,2");
  assertEquals(p.has("b"), true);
  assertEquals(p.has("z"), false);
});

test("URLSearchParams set/append/delete and serialization", () => {
  const p = new URLSearchParams();
  p.append("k", "v 1");
  p.append("k", "v2");
  assertEquals(p.toString(), "k=v+1&k=v2");
  p.set("k", "only");
  assertEquals(p.toString(), "k=only");
  p.delete("k");
  assertEquals(p.toString(), "");
});

test("URL.searchParams reflects the query", () => {
  const u = new URL("https://h.test/?a=1");
  u.searchParams.append("b", "2");
  assertEquals(u.search, "?a=1&b=2");
});
