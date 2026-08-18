import { assert, assertEquals, test } from "runtime:test";
import { HttpError, json, securityHeaders, toResponse } from "./http.ts";

test("json carries its type and the security headers", async () => {
  const response = json({ a: 1 });
  assertEquals(response.headers.get("content-type"), "application/json; charset=utf-8");
  assertEquals(response.headers.get("x-content-type-options"), "nosniff");
  assertEquals(await response.text(), '{"a":1}');
});

test("an HttpError becomes its own status and message", async () => {
  const { response, unexpected } = toResponse(HttpError.notFound("No such record"));
  assertEquals(response.status, 404);
  assertEquals(unexpected, false);
  assertEquals((await response.json()).error, "No such record");
});

test("a bad request can name the field that was wrong", async () => {
  const { response } = toResponse(HttpError.badRequest("Invalid", { title: "must not be empty" }));
  assertEquals(response.status, 400);
  assertEquals((await response.json()).details.title, "must not be empty");
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

test("nothing is framed and nothing is loaded", () => {
  const headers = securityHeaders();
  assert(headers["content-security-policy"]!.includes("frame-ancestors 'none'"));
  assertEquals(headers["referrer-policy"], "no-referrer");
});
