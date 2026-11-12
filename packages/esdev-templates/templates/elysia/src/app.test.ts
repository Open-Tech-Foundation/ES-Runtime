import { expect, test } from "runtime:test";
import { createApp } from "./app.ts";

function get(path: string): Promise<Response> {
  return createApp().handle(new Request(`https://app.test${path}`));
}

test("GET /api/hello answers the greeting", async () => {
  const res = await get("/api/hello");
  expect(res.status).toBe(200);
  expect(await res.json()).toEqual({ message: "Hello, world!" });
});

test("GET /api/greet greets a valid name", async () => {
  const res = await get("/api/greet?name=you");
  expect(res.status).toBe(200);
  expect(await res.json()).toEqual({ message: "Hello, you!" });
});

test("GET /api/greet refuses a missing name", async () => {
  const res = await get("/api/greet");
  expect(res.status).toBe(422);
});

test("GET / renders the page", async () => {
  const res = await get("/");
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toContain("text/html");
  expect(await res.text()).toContain("src/app.ts");
});
