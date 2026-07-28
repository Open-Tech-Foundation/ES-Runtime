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

test("Headers constructor length matches WebIDL", () => {
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

test("Request rejects a body on GET or HEAD", () => {
  assertThrows(() => new Request("https://a.example/", { method: "GET", body: "x" }), "TypeError");
  assertThrows(() => new Request("https://a.example/", { method: "head", body: "x" }), "TypeError");
  // A body on any other method is fine.
  assertEquals(new Request("https://a.example/", { method: "POST", body: "x" }).method, "POST");
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

test("Request exposes the standard mode/credentials/redirect defaults", () => {
  const r = new Request("https://a.example/");
  assertEquals(r.redirect, "follow");
  assertEquals(r.credentials, "same-origin");
  assertEquals(r.mode, "cors");
  assertEquals(r.cache, "default");
  assertEquals(r.referrer, "about:client");
  assertEquals(r.integrity, "");
  assertEquals(r.keepalive, false);
});

test("Request exposes formData()", () => {
  assertEquals(typeof Request.prototype.formData, "function");
});

test("Request init overrides the policy defaults and clone carries them", () => {
  const r = new Request("https://a.example/", {
    redirect: "manual",
    credentials: "include",
    mode: "no-cors",
    integrity: "sha256-abc",
    keepalive: true,
  });
  assertEquals(r.redirect, "manual");
  assertEquals(r.credentials, "include");
  assertEquals(r.mode, "no-cors");
  assertEquals(r.integrity, "sha256-abc");
  assertEquals(r.keepalive, true);
  assertEquals(r.clone().redirect, "manual");
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

test("Response.json sets a JSON Content-Type unless init supplied one", () => {
  assertEquals(Response.json({ a: 1 }).headers.get("content-type"), "application/json");
  assertEquals(
    Response.json({ a: 1 }, { headers: { "content-type": "application/problem+json" } })
      .headers.get("content-type"),
    "application/problem+json",
  );
});

test("Response.json rejects a value JSON cannot serialise", () => {
  assertThrows(() => Response.json(undefined), "TypeError");
});

test("Response body is a ReadableStream and null for a null-body status", () => {
  assert(new Response("x").body instanceof ReadableStream);
  assertEquals(new Response(null, { status: 204 }).body, null);
});

test("Response rejects a status outside 200-599", () => {
  assertThrows(() => new Response("x", { status: 999 }), "RangeError");
  assertThrows(() => new Response("x", { status: 99 }), "RangeError");
  assertThrows(() => new Response("x", { status: 200.5 }), "RangeError");
  // The boundaries themselves are fine.
  assertEquals(new Response("x", { status: 200 }).status, 200);
  assertEquals(new Response("x", { status: 599 }).status, 599);
});

test("Response rejects a body on a null-body status", () => {
  // 101 and 103 are also null-body statuses but sit outside the constructor's
  // 200-599 range, so a script cannot reach them here at all.
  for (const status of [204, 205, 304]) {
    assertThrows(() => new Response("x", { status }), "TypeError");
    // The same status with no body is allowed.
    assertEquals(new Response(null, { status }).status, status);
  }
});

test("Response.error() produces an error-typed response", () => {
  const r = Response.error();
  assertEquals(r.type, "error");
  assertEquals(r.status, 0);
  assertEquals(r.ok, false);
});

test("Response.redirect() produces a redirect response", () => {
  const r = Response.redirect("https://a.example/", 302);
  assertEquals(r.status, 302);
  assertEquals(r.headers.get("location"), "https://a.example/");
  // Defaults to 302, and only redirect statuses are accepted.
  assertEquals(Response.redirect("https://a.example/").status, 302);
  for (const status of [301, 303, 307, 308]) {
    assertEquals(Response.redirect("https://a.example/", status).status, status);
  }
  assertThrows(() => Response.redirect("https://a.example/", 200), "RangeError");
  assertThrows(() => Response.redirect("not a url"), "TypeError");
});

test("a cloned Response keeps its type", () => {
  assertEquals(new Response("x").clone().type, "default");
});

test("Response exposes formData()", () => {
  assertEquals(typeof Response.prototype.formData, "function");
});

test("formData() parses application/x-www-form-urlencoded", async () => {
  const r = new Response("a=1&b=two+words&a=3", {
    headers: { "content-type": "application/x-www-form-urlencoded" },
  });
  const fd = await r.formData();
  assertEquals(fd.getAll("a").join(","), "1,3");
  assertEquals(fd.get("b"), "two words");
});

test("formData() round-trips a multipart body built by FormData", async () => {
  const sent = new FormData();
  sent.append("field", "plain value");
  sent.append("file", new File(["file contents"], "note.txt", { type: "text/plain" }));
  const parsed = await new Response(sent).formData();
  assertEquals(parsed.get("field"), "plain value");
  const f = parsed.get("file");
  assert(f instanceof File);
  assertEquals(f.name, "note.txt");
  assertEquals(f.type, "text/plain");
  assertEquals(await f.text(), "file contents");
});

test("formData() rejects a body it cannot parse", async () => {
  let name = null;
  try {
    await new Response("x", { headers: { "content-type": "text/plain" } }).formData();
  } catch (e) {
    name = e.name;
  }
  assertEquals(name, "TypeError");
});
