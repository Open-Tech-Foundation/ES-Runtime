// Hello-world HTTP server, one file that runs on every runtime — the classic
// "requests/sec" shape (à la the Bun/TechEmpower plaintext benchmark). It binds
// loopback and stays up; an external load generator (oha, run by rps.sh)
// measures throughput. An HTTP server is not a shared Web API, so each runtime
// uses its own surface.
//
// The port comes from BENCH_PORT — rps.sh picks a free one per run, so a dev
// server on the old fixed :3000 can no longer be load-tested in our place.
const BODY = "Hello, World!";
const PORT = Number(await benchPort()) || 3000;

// Reading the environment is the one thing with no shared spelling: Deno gates
// it behind Deno.env, esrun behind the capability-checked runtime:process, and
// Node/Bun expose process.env. Deno is checked first because Deno 2 also
// provides a `process` global.
async function benchPort() {
  if (typeof Deno !== "undefined") return Deno.env.get("BENCH_PORT");
  if (typeof process !== "undefined" && process.env) return process.env.BENCH_PORT;
  const { env } = await import("runtime:process");
  return env.BENCH_PORT;
}

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
