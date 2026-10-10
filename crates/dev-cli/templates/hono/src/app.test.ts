import { expect, test } from "runtime:test";
import { createApp } from "./app.tsx";

test("GET /api/hello answers the greeting", async () => {
  const res = await createApp().request("/api/hello");
  expect(res.status).toBe(200);
  expect(await res.json()).toEqual({ message: "Hello, world!" });
});

test("GET /api/greet greets a valid name", async () => {
  const res = await createApp().request("/api/greet?name=you");
  expect(res.status).toBe(200);
  expect(await res.json()).toEqual({ message: "Hello, you!" });
});

test("GET /api/greet refuses a missing name", async () => {
  const res = await createApp().request("/api/greet");
  expect(res.status).toBe(400);
});

test("GET / renders the page", async () => {
  const res = await createApp().request("/");
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toContain("text/html");
  expect(await res.text()).toContain("src/app.tsx");
});
