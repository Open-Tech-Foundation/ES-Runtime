// Hello-world HTTP server, one file that runs on every runtime — the classic
// "requests/sec" shape (à la the Bun/TechEmpower plaintext benchmark). It binds
// loopback and stays up; an external load generator (oha, run by rps.sh)
// measures throughput. An HTTP server is not a shared Web API, so each runtime
// uses its own surface.
//
// The port comes from BENCH_PORT — rps.sh picks a free one per run, so a dev
// server on the old fixed :3000 can no longer be load-tested in our place.
//
// BENCH_H2=1 (set by http2.sh) asks for a cleartext HTTP/2 server. Node and Bun
// need to be told, because for both of them h2c lives behind the separate
// `node:http2` API — `node:http`'s server and `Bun.serve` are HTTP/1.1-only —
// while esrun and Deno detect the version per connection on the one server they
// already have. That asymmetry is the point of the flag, not a wart in it: the
// h2 column measures each runtime's *best available* cleartext h2 server, which
// for two of the four is a different server than the h1 column used.
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

const NODE_LIKE =
  typeof Bun !== "undefined" ||
  (typeof process !== "undefined" && process.versions && process.versions.node);

if (typeof Deno !== "undefined") {
  Deno.serve(
    { hostname: "127.0.0.1", port: PORT, onListen() {} },
    () => new Response(BODY),
  );
} else if (H2 && NODE_LIKE) {
  // `http2.createServer` is h2c-only — it does not also answer HTTP/1.1 — so
  // Node and Bun measure the two versions from two servers where the others use
  // one. Bun implements this API too, so its h2 number is a real measurement
  // rather than the n/a that `Bun.serve` alone would produce.
  const http2 = await import("node:http2");
  http2
    .createServer((_req, res) => {
      res.setHeader("content-type", "text/plain");
      res.end(BODY);
    })
    .listen(PORT, "127.0.0.1");
} else if (typeof Bun !== "undefined") {
  Bun.serve({ hostname: "127.0.0.1", port: PORT, fetch: () => new Response(BODY) });
} else if (NODE_LIKE) {
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
