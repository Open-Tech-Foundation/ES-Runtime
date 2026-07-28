// URLPattern (SPEC §2.4) — the path-to-regexp dialect the standard adopts.

test("a named group matches one segment and is captured", () => {
  const p = new URLPattern({ pathname: "/u/:id" });
  assertEquals(p.test("https://x.test/u/123"), true);
  assertEquals(p.exec("https://x.test/u/123").pathname.groups.id, "123");
  // A segment group does not cross the delimiter.
  assertEquals(p.test("https://x.test/u/1/2"), false);
  assertEquals(p.test("https://x.test/u/"), false);
});

test("a group can carry a custom regex", () => {
  const named = new URLPattern({ pathname: "/u/:id(\\d+)" });
  assertEquals(named.test("https://x.test/u/12"), true);
  assertEquals(named.test("https://x.test/u/ab"), false);
  assertEquals(named.exec("https://x.test/u/12").pathname.groups.id, "12");

  // Anonymous groups are captured by index.
  const anon = new URLPattern({ pathname: "/u/(\\d+)" });
  assertEquals(anon.test("https://x.test/u/12"), true);
  assertEquals(anon.test("https://x.test/u/ab"), false);
  assertEquals(anon.exec("https://x.test/u/12").pathname.groups["0"], "12");
});

test("the ? modifier makes a segment optional and absorbs its separator", () => {
  const p = new URLPattern({ pathname: "/a/:b?" });
  assertEquals(p.test("https://x.test/a"), true);
  assertEquals(p.test("https://x.test/a/x"), true);
  assertEquals(p.test("https://x.test/a/x/y"), false);
  // An unmatched optional group is undefined, not "".
  assertEquals(p.exec("https://x.test/a").pathname.groups.b, undefined);
  assertEquals(p.exec("https://x.test/a/x").pathname.groups.b, "x");
});

test("the + and * modifiers repeat a segment", () => {
  const plus = new URLPattern({ pathname: "/a/:rest+" });
  assertEquals(plus.test("https://x.test/a"), false);
  assertEquals(plus.test("https://x.test/a/x"), true);
  assertEquals(plus.test("https://x.test/a/x/y/z"), true);
  assertEquals(plus.exec("https://x.test/a/x/y/z").pathname.groups.rest, "x/y/z");

  const star = new URLPattern({ pathname: "/a/:rest*" });
  assertEquals(star.test("https://x.test/a"), true);
  assertEquals(star.test("https://x.test/a/x/y"), true);
});

test("a full wildcard crosses delimiters and is captured by index", () => {
  const p = new URLPattern({ pathname: "/f/*" });
  assertEquals(p.test("https://x.test/f/deep/path"), true);
  assertEquals(p.exec("https://x.test/f/deep/path").pathname.groups["0"], "deep/path");
  // A wildcard can sit mid-pattern.
  assertEquals(new URLPattern({ pathname: "/*/end" }).test("https://x.test/a/b/end"), true);
});

test("a {…} group lets a modifier cover literal text", () => {
  const p = new URLPattern({ pathname: "/books{/old}?" });
  assertEquals(p.test("https://x.test/books"), true);
  assertEquals(p.test("https://x.test/books/old"), true);
  assertEquals(p.test("https://x.test/books/new"), false);
  assertEquals(new URLPattern({ protocol: "http{s}?" }).test("https://x.test/"), true);
  assertEquals(new URLPattern({ protocol: "http{s}?" }).test("http://x.test/"), true);
  assertEquals(new URLPattern({ protocol: "http{s}?" }).test("ftp://x.test/"), false);
});

test("a backslash escapes a pattern character", () => {
  assertEquals(new URLPattern({ pathname: "/e\\:sc" }).test("https://x.test/e:sc"), true);
  // A literal dot is not a regex wildcard.
  const dot = new URLPattern({ pathname: "/a.b" });
  assertEquals(dot.test("https://x.test/a.b"), true);
  assertEquals(dot.test("https://x.test/axb"), false);
});

test("hostname groups are bounded by '.' rather than '/'", () => {
  assertEquals(new URLPattern({ hostname: ":sub.example.com" }).test("https://api.example.com/"), true);
  assertEquals(new URLPattern({ hostname: ":sub.example.com" }).test("https://a.b.example.com/"), false);
  assertEquals(new URLPattern({ hostname: "*.example.com" }).test("https://a.b.example.com/"), true);
});

test("every component can be matched", () => {
  assertEquals(new URLPattern({ port: "8080" }).test("https://x.test:8080/"), true);
  assertEquals(new URLPattern({ port: "8080" }).test("https://x.test:9090/"), false);
  assertEquals(new URLPattern({ search: "q=:term" }).test("https://x.test/?q=hi"), true);
  assertEquals(new URLPattern({ hash: "sec-:n" }).test("https://x.test/#sec-3"), true);
  assertEquals(new URLPattern({ protocol: "https" }).test("https://x.test/"), true);
});

test("an unspecified component matches anything", () => {
  const p = new URLPattern({});
  assertEquals(p.test("https://x.test/anything?q=1#h"), true);
  assertEquals(p.pathname, "*");
  assertEquals(p.protocol, "*");
});

test("a string pattern splits into components", () => {
  const p = new URLPattern("https://x.test/u/:id?q=1#f");
  assertEquals(p.protocol, "https");
  assertEquals(p.hostname, "x.test");
  assertEquals(p.pathname, "/u/:id");
  assertEquals(p.search, "q=1");
  assertEquals(p.hash, "f");
  assertEquals(p.test("https://x.test/u/9?q=1#f"), true);
});

test("exec reports inputs and per-component input, and test rejects a bad URL", () => {
  const p = new URLPattern({ pathname: "/u/:id" });
  const r = p.exec("https://x.test/u/5");
  assertEquals(r.inputs.length, 1);
  assertEquals(r.inputs[0], "https://x.test/u/5");
  assertEquals(r.pathname.input, "/u/5");
  assertEquals(r.hostname.input, "x.test");
  assertEquals(p.test("not a url"), false);
  assertEquals(p.exec("not a url"), null);
  assertEquals(p.exec("https://x.test/nope"), null);
});

test("hasRegExpGroups reports whether a custom regex is used", () => {
  assertEquals(new URLPattern({ pathname: "/u/:id(\\d+)" }).hasRegExpGroups, true);
  assertEquals(new URLPattern({ pathname: "/u/:id" }).hasRegExpGroups, false);
  assertEquals(new URLPattern({ pathname: "/u/*" }).hasRegExpGroups, false);
});

test("ignoreCase makes matching case-insensitive", () => {
  assertEquals(new URLPattern({ pathname: "/Case" }).test("https://x.test/case"), false);
  assertEquals(
    new URLPattern({ pathname: "/Case" }, { ignoreCase: true }).test("https://x.test/case"),
    true,
  );
});

test("a malformed pattern is rejected at construction", () => {
  assertThrows(() => new URLPattern({ pathname: "/:" }), "TypeError");
  assertThrows(() => new URLPattern({ pathname: "/a{b" }), "TypeError");
  assertThrows(() => new URLPattern({ pathname: "/a}" }), "TypeError");
  assertThrows(() => new URLPattern({ pathname: "/a()" }), "TypeError");
  assertThrows(() => new URLPattern({ pathname: "/a(" }), "TypeError");
});

test("a pattern resolves against a base URL", () => {
  const p = new URLPattern({ pathname: "/u/:id" }, "https://x.test/");
  assertEquals(p.hostname, "x.test");
  assertEquals(p.test("https://x.test/u/1"), true);
  assertEquals(p.test("https://other.test/u/1"), false);
});
