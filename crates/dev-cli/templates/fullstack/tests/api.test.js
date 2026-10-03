import { expect, test } from "runtime:test";
import { GET } from "../app/api/hello/route.js";
import loader from "../app/loader.js";

test("GET /api/hello answers JSON", async () => {
  const response = await GET(new Request("http://localhost/api/hello"));
  expect(response.status).toBe(200);
  expect(await response.json()).toEqual({ message: "Hello, world!" });
});

test("the root loader feeds the page", async () => {
  expect(await loader({ params: {}, query: {} })).toEqual({ message: "Hello, world!" });
});
