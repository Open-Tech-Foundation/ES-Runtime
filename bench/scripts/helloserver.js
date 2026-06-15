// Hello-world HTTP server, one file that runs on every runtime — the classic
// "requests/sec" shape (à la the Bun/TechEmpower plaintext benchmark). It binds
// :3000 on loopback and stays up; an external load generator (autocannon, run
// by rps.sh) measures throughput. An HTTP server is not a shared Web API, so
// each runtime uses its own surface.
const BODY = "Hello, World!";
const PORT = 3000;

if (typeof Deno !== "undefined") {
  Deno.serve(
    { hostname: "127.0.0.1", port: PORT, onListen() {} },
    () => new Response(BODY),
  );
} else if (typeof Bun !== "undefined") {
  Bun.serve({ hostname: "127.0.0.1", port: PORT, fetch: () => new Response(BODY) });
} else if (typeof process !== "undefined" && process.versions && process.versions.node) {
  const http = await import("node:http");
  http
    .createServer((_req, res) => {
      res.setHeader("content-type", "text/plain");
      res.end(BODY);
    })
    .listen(PORT, "127.0.0.1");
} else {
  const { serve } = await import("runtime:http");
  serve({ hostname: "127.0.0.1", port: PORT }, () => new Response(BODY));
}
