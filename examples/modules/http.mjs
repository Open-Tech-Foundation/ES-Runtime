// runtime:http — an HTTP/1.1 server. Run with:
//   esrun examples/modules/http.mjs
// The handler takes a web Request and returns a web Response (the same Fetch
// API objects fetch uses).
import { serve } from "runtime:http";

const server = serve({ hostname: "127.0.0.1", port: 0 }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/echo" && request.method === "POST") {
    return new Response(await request.text(), { status: 200 });
  }
  return Response.json({ method: request.method, path: url.pathname });
});

const { hostname, port } = await server.addr;
console.log(`listening on http://${hostname}:${port}`);

// Drive it once with fetch, then shut down so the process exits.
const res = await fetch(`http://127.0.0.1:${port}/echo`, {
  method: "POST",
  body: "hello http",
});
console.log("status:", res.status);
console.log("body:", await res.text());

await server.stop();
