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

test("URLSearchParams iteration methods work correctly", () => {
  const p = new URLSearchParams("a=1&b=2");
  assertEquals([...p.keys()].join(","), "a,b");
  assertEquals([...p.values()].join(","), "1,2");
  assertEquals([...p.entries()].map(([k, v]) => `${k}:${v}`).join(","), "a:1,b:2");
});

test("URLSearchParams handles constructor with sequence of pairs", () => {
  const p = new URLSearchParams([["x", "10"], ["y", "20"]]);
  assertEquals(p.get("x"), "10");
  assertEquals(p.get("y"), "20");
});


test("URLSearchParams leaves '*' unescaped per the urlencoded safe set", () => {
  assertEquals(new URLSearchParams([["a", "*"]]).toString(), "a=*");
});

test("URLSearchParams accepts any iterable of pairs, not just arrays", () => {
  assertEquals(new URLSearchParams(new Map([["a", "1"]])).toString(), "a=1");
});

test("URL.parse returns null instead of throwing", () => {
  assertEquals(URL.parse("::::"), null);
  assertEquals(URL.parse("https://a.example/").href, "https://a.example/");
});

test("URLSearchParams escapes the rest of the unsafe punctuation", () => {
  assertEquals(new URLSearchParams([["a", "!'()~"]]).toString(), "a=%21%27%28%29%7E");
});

test("URLSearchParams accepts a generator of pairs and rejects bad arity", () => {
  function* pairs() {
    yield ["a", "1"];
    yield ["b", "2"];
  }
  assertEquals(new URLSearchParams(pairs()).toString(), "a=1&b=2");
  assertThrows(() => new URLSearchParams([["only"]]), "TypeError");
});

test("URL.parse resolves against a base", () => {
  assertEquals(URL.parse("/p", "https://a.example/x").href, "https://a.example/p");
  assertEquals(URL.parse("/p", "not a base"), null);
});

test("a username with no password reads back an empty password", () => {
  const u = new URL("https://foo@example.com/p");
  assertEquals(u.username, "foo");
  assertEquals(u.password, "");
  assertEquals(u.hostname, "example.com");
  assertEquals(u.href, "https://foo@example.com/p");
});

test("every credential shape round-trips", () => {
  const cases = [
    ["https://u:p@example.com/", "u", "p"],
    ["https://u@example.com/", "u", ""],
    ["https://:p@example.com/", "", "p"],
    ["https://example.com/", "", ""],
  ];
  for (const [href, username, password] of cases) {
    const u = new URL(href);
    assertEquals(u.username, username, href);
    assertEquals(u.password, password, href);
  }
});
