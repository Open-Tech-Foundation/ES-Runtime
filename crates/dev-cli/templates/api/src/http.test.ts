import { test, assert, assertEquals, assertRejects } from "runtime:test";
import { HttpError, json, noContent, readJson, securityHeaders, toResponse } from "./http.ts";

test("json carries its type and the security headers", async () => {
  const response = json({ a: 1 });
  assertEquals(response.headers.get("content-type"), "application/json; charset=utf-8");
  assertEquals(response.headers.get("x-content-type-options"), "nosniff");
  assertEquals(await response.text(), '{"a":1}');
});

test("an HttpError becomes its own status and message", async () => {
  const { response, unexpected } = toResponse(HttpError.notFound("No task 7"));
  assertEquals(response.status, 404);
  assertEquals(unexpected, false);
  assertEquals((await response.json()).error, "No task 7");
});

test("anything else is a flat 500 that says nothing", async () => {
  // A bug's message names hostnames, paths and sometimes the data. It belongs
  // in the log, not in the response.
  const { response, unexpected } = toResponse(new Error("connect ECONNREFUSED 10.0.0.5:5432"));
  assertEquals(response.status, 500);
  assertEquals(unexpected, true);
  const body = await response.text();
  assert(!body.includes("10.0.0.5"), body);
  assertEquals(JSON.parse(body).error, "Internal Server Error");
});

test("a 422 carries the field that was wrong", async () => {
  const { response } = toResponse(HttpError.invalid({ title: "must not be empty" }));
  assertEquals(response.status, 422);
  assertEquals((await response.json()).details.title, "must not be empty");
});

test("204 has no body", async () => {
  const response = noContent();
  assertEquals(response.status, 204);
  assertEquals(await response.text(), "");
});

test("a body that is not JSON is refused before it is parsed", async () => {
  const form = new Request("http://x/", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: "title=hi",
  });
  await assertRejects(() => readJson(form));
});

test("a malformed JSON body is a 400, not a crash", async () => {
  const broken = new Request("http://x/", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{ not json",
  });
  await assertRejects(() => readJson(broken));
});

test("a well-formed body comes back parsed", async () => {
  const request = new Request("http://x/", {
    method: "POST",
    headers: { "content-type": "application/json; charset=utf-8" },
    body: '{"title":"hi"}',
  });
  assertEquals((await readJson(request) as { title: string }).title, "hi");
});

test("nothing is framed and nothing is loaded", () => {
  const headers = securityHeaders();
  assert(headers["content-security-policy"]!.includes("frame-ancestors 'none'"));
  assertEquals(headers["referrer-policy"], "no-referrer");
});
