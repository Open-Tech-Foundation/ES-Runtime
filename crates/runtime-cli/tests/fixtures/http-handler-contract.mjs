// The two halves of `serve`'s documented failure contract, end to end against
// the real server: a handler that throws and a handler that returns something
// other than a Response both become a 500.
//
// The non-Response case used to be coerced with `String(value)` and sent as a
// **200**, so `return { ok: true }` shipped the body "[object Object]" with a
// success status — a handler bug delivered to the client as a working response.
import { serve } from "runtime:http";

const cases = {
  "/throw": () => {
    throw new Error("handler blew up");
  },
  "/reject": async () => {
    throw new TypeError("async handler blew up");
  },
  "/string": () => "plain string",
  "/object": () => ({ ok: true }),
  "/null": () => null,
  "/undefined": () => undefined,
  "/ok": () => new Response("fine"),
};

const server = serve({ port: 0, hostname: "127.0.0.1" }, (req) =>
  cases[new URL(req.url).pathname](),
);
const { port } = await server.addr;

for (const path of Object.keys(cases)) {
  const r = await fetch(`http://127.0.0.1:${port}${path}`);
  const body = await r.text();
  console.log(`${path} status:${r.status} body:${JSON.stringify(body)}`);
}

// A handler bug must not leak its detail to the client — the reason goes to the
// developer (stderr), the client gets a bare 500.
const leak = await fetch(`http://127.0.0.1:${port}/throw`).then((r) => r.text());
console.log(`leak:${leak.includes("blew up")}`);

await server.stop();
console.log("HANDLER_CONTRACT_OK");
