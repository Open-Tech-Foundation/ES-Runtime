// Hello-world HTTP server, one file that runs on every runtime — the classic
// "requests/sec" shape (à la the Bun/TechEmpower plaintext benchmark). It binds
// loopback and stays up; an external load generator (oha, run by rps.sh)
// measures throughput. An HTTP server is not a shared Web API, so each runtime
// uses its own surface.
//
// The port comes from BENCH_PORT — rps.sh picks a free one per run, so a dev
// server on the old fixed :3000 can no longer be load-tested in our place.
//
// BENCH_H2=1 (set by http2.sh) asks for a cleartext HTTP/2 server. Only Node
// needs to be told: its `node:http` server is HTTP/1.1-only and h2c lives
// behind a separate `node:http2` API, where every other runtime here detects
// the version per connection on one server. That asymmetry is the point of the
// flag, not a wart in it.
const BODY = "Hello, World!";
const PORT = Number(await benchEnv("BENCH_PORT")) || 3000;
const H2 = (await benchEnv("BENCH_H2")) === "1";

// Reading the environment is the one thing with no shared spelling: Deno gates
// it behind Deno.env, esrun behind the capability-checked runtime:process, and
// Node/Bun expose process.env. Deno is checked first because Deno 2 also
// provides a `process` global.
async function benchEnv(name) {
  if (typeof Deno !== "undefined") return Deno.env.get(name);
  if (typeof process !== "undefined" && process.env) return process.env[name];
  const { env } = await import("runtime:process");
  return env[name];
}

if (typeof Deno !== "undefined") {
  Deno.serve(
    { hostname: "127.0.0.1", port: PORT, onListen() {} },
    () => new Response(BODY),
  );
} else if (typeof Bun !== "undefined") {
  Bun.serve({ hostname: "127.0.0.1", port: PORT, fetch: () => new Response(BODY) });
} else if (typeof process !== "undefined" && process.versions && process.versions.node) {
  // `http2.createServer` is h2c-only — it does not also answer HTTP/1.1 — so
  // Node measures the two versions from two servers where the others use one.
  const http = await import(H2 ? "node:http2" : "node:http");
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
