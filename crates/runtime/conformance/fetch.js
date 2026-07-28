// WHATWG Fetch — the Headers / Request / Response object surface. No network
// is touched here; transport behaviour is covered by the runtime's own tests.
//
// Cases still written as `todo` are known deviations; see RESULTS.md.

// ---- Headers --------------------------------------------------------------

test("Headers are case-insensitive and iterate lower-cased in sorted order", () => {
  const h = new Headers({ "X-B": "2", "x-a": "1" });
  assertEquals(h.get("X-A"), "1");
  assertEquals([...h.keys()].join(","), "x-a,x-b");
});

test("Headers.append combines duplicate values with ', '", () => {
  const h = new Headers({ "x-a": "1" });
  h.append("x-a", "3");
  assertEquals(h.get("x-a"), "1, 3");
});

test("Headers.get returns null for a missing name", () => {
  assertEquals(new Headers().get("nope"), null);
});

test("Headers reject an invalid header name", () => {
  assertThrows(() => new Headers().set("a b", "v"), "TypeError");
});

test("Headers strip leading and trailing whitespace from values", () => {
  const h = new Headers();
  h.set("a", "  v  ");
  assertEquals(h.get("a"), "v");
});

test("getSetCookie returns each Set-Cookie separately", () => {
  const h = new Headers();
  h.append("set-cookie", "a=1");
  h.append("set-cookie", "b=2");
  assertEquals(h.getSetCookie().length, 2);
});

test("Headers reject a value containing NUL, CR or LF", () => {
  assertThrows(() => new Headers().set("a", "v\n1"), "TypeError");
  assertThrows(() => new Headers().set("a", "v\r1"), "TypeError");
  assertThrows(() => new Headers().set("a", "v\u00001"), "TypeError");
  assertThrows(() => new Headers().append("a", "v\r\nX-Evil: 1"), "TypeError");
  // A value that is only whitespace still normalises to the empty string.
  const h = new Headers();
  h.set("a", "\r\n");
  assertEquals(h.get("a"), "");
});

todo("Headers constructor length matches WebIDL", () => {
  assertEquals(Headers.length, 0);
});

// ---- Request --------------------------------------------------------------

test("Request normalises the method and serialises the URL", () => {
  const r = new Request("https://a.example/p", { method: "post" });
  assertEquals(r.method, "POST");
  assertEquals(r.url, "https://a.example/p");
  assertEquals(r.bodyUsed, false);
});

test("Request rejects a relative URL with no base", () => {
  assertThrows(() => new Request("/p"), "TypeError");
});

todo("Request rejects a body on GET or HEAD", () => {
  assertThrows(() => new Request("https://a.example/", { method: "GET", body: "x" }), "TypeError");
});

test("Request exposes a signal, defaulting to a fresh unaborted one", () => {
  const r = new Request("https://a.example/");
  assert(r.signal instanceof AbortSignal);
  assertEquals(r.signal.aborted, false);
});

test("Request adopts the signal from init", () => {
  const c = new AbortController();
  assertEquals(new Request("https://a.example/", { signal: c.signal }).signal, c.signal);
});

test("Request rejects a non-AbortSignal signal", () => {
  assertThrows(() => new Request("https://a.example/", { signal: {} }), "TypeError");
});

test("a cloned Request keeps the original's signal", () => {
  const c = new AbortController();
  const r = new Request("https://a.example/", { signal: c.signal });
  assertEquals(r.clone().signal, c.signal);
});

todo("Request exposes the standard mode/credentials/redirect defaults", () => {
  const r = new Request("https://a.example/");
  assertEquals(r.redirect, "follow");
  assertEquals(r.credentials, "same-origin");
  assertEquals(r.mode, "cors");
  assertEquals(r.cache, "default");
  assertEquals(r.referrer, "about:client");
  assertEquals(r.integrity, "");
  assertEquals(r.keepalive, false);
});

todo("Request exposes formData()", () => {
  assertEquals(typeof Request.prototype.formData, "function");
});

// ---- Response -------------------------------------------------------------

test("Response reports status, ok and the default type", () => {
  const r = new Response("hi", { status: 201 });
  assertEquals(r.status, 201);
  assertEquals(r.ok, true);
  assertEquals(r.statusText, "");
  assertEquals(r.type, "default");
  assertEquals(r.url, "");
  assertEquals(r.redirected, false);
});

test("Response infers Content-Type from the body type", () => {
  assertEquals(new Response("x").headers.get("content-type"), "text/plain;charset=UTF-8");
  assertEquals(
    new Response(new URLSearchParams("a=1")).headers.get("content-type"),
    "application/x-www-form-urlencoded;charset=UTF-8",
  );
  assertEquals(
    new Response(new Blob(["x"], { type: "application/x-thing" })).headers.get("content-type"),
    "application/x-thing",
  );
});

test("Response.json defaults to status 200", () => {
  assertEquals(Response.json({ a: 1 }).status, 200);
});

todo("Response.json sets a JSON Content-Type", () => {
  // The string body's inferred "text/plain;charset=UTF-8" is already present,
  // so the `has("content-type")` guard in Response.json never fires.
  assertEquals(Response.json({ a: 1 }).headers.get("content-type"), "application/json");
});

test("Response body is a ReadableStream and null for a null-body status", () => {
  assert(new Response("x").body instanceof ReadableStream);
  assertEquals(new Response(null, { status: 204 }).body, null);
});

todo("Response rejects a status outside 200-599", () => {
  assertThrows(() => new Response("x", { status: 999 }), "RangeError");
  assertThrows(() => new Response("x", { status: 99 }), "RangeError");
});

todo("Response rejects a body on a null-body status", () => {
  assertThrows(() => new Response("x", { status: 204 }), "TypeError");
});

todo("Response.error() produces an error-typed response", () => {
  const r = Response.error();
  assertEquals(r.type, "error");
  assertEquals(r.status, 0);
});

todo("Response.redirect() produces a redirect response", () => {
  const r = Response.redirect("https://a.example/", 302);
  assertEquals(r.status, 302);
  assertEquals(r.headers.get("location"), "https://a.example/");
});

todo("Response exposes formData()", () => {
  assertEquals(typeof Response.prototype.formData, "function");
});
