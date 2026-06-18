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

test("URL hostname setter handles ports correctly", () => {
  const u = new URL("http://example.com:8080");
  u.hostname = "test.com:9999"; // Fails parsing, ignored
  assertEquals(u.href, "http://example.com:8080/");
  
  u.hostname = "test.com"; // Succeeds
  assertEquals(u.href, "http://test.com:8080/");
  
  u.hostname = "[::1]:80"; // Fails parsing, ignored
  assertEquals(u.href, "http://test.com:8080/");
  
  u.hostname = "[::1]"; // Succeeds
  assertEquals(u.href, "http://[::1]:8080/");
});

test("URL host setter parses and sets ports", () => {
  const u1 = new URL("http://example.com:8080");
  u1.host = "test.com:9999"; // Succeeds, sets both
  assertEquals(u1.href, "http://test.com:9999/");
  
  const u2 = new URL("http://example.com:8080");
  u2.host = "test.com"; // Succeeds, leaves port alone
  assertEquals(u2.href, "http://test.com:8080/");
  
  const u3 = new URL("http://example.com:8080");
  u3.host = "test.com:"; // Empty port — host changes, existing port kept
  assertEquals(u3.href, "http://test.com:8080/");
  
  const u4 = new URL("http://example.com:8080");
  u4.host = "[::1]:80"; // Default port dropped
  assertEquals(u4.href, "http://[::1]/");

  // Invalid ports fail the whole setter, ignoring
  const u5 = new URL("http://example.com:8080");
  u5.host = "test.com:abc"; // Invalid port fails, ignored
  assertEquals(u5.href, "http://example.com:8080/");
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
