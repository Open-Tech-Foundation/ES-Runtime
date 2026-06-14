// HTTP server benchmark: each runtime starts its own native HTTP server on an
// ephemeral loopback port, then the workload self-times N requests against it,
// fired in batches of C concurrent (the way a server is actually driven —
// throughput under load, not one-at-a-time latency). An HTTP server is not a
// shared Web API, so each runtime uses its own surface (like the fs workloads).
// The handler returns a small fixed body; this measures the warm request/
// response path (connection reuse), not connection setup.
(async () => {
  const N = 2000;
  const C = 100;
  const BODY = "x".repeat(64);

  // start() → { port, stop } for the host runtime's native server API.
  let start;
  if (typeof Deno !== "undefined") {
    start = async () => {
      const server = Deno.serve(
        { hostname: "127.0.0.1", port: 0, onListen() {} },
        () => new Response(BODY, { headers: { "content-type": "text/plain" } }),
      );
      return { port: server.addr.port, stop: () => server.shutdown() };
    };
  } else if (typeof Bun !== "undefined") {
    start = async () => {
      const server = Bun.serve({
        hostname: "127.0.0.1",
        port: 0,
        fetch: () => new Response(BODY, { headers: { "content-type": "text/plain" } }),
      });
      return { port: server.port, stop: () => server.stop(true) };
    };
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const http = await import("node:http");
    start = async () => {
      const server = http.createServer((req, res) => {
        res.setHeader("content-type", "text/plain");
        res.end(BODY);
      });
      await new Promise((r) => server.listen(0, "127.0.0.1", r));
      return {
        port: server.address().port,
        stop: () => new Promise((r) => server.close(r)),
      };
    };
  } else {
    const { serve } = await import("runtime:http");
    start = async () => {
      const server = serve(
        { hostname: "127.0.0.1", port: 0 },
        () => new Response(BODY, { headers: { "content-type": "text/plain" } }),
      );
      const { port } = await server.addr;
      return { port, stop: () => server.stop() };
    };
  }

  const { port, stop } = await start();
  const url = `http://127.0.0.1:${port}/`;

  const batch = async (c) => {
    const ps = [];
    for (let i = 0; i < c; i++) ps.push(fetch(url).then((r) => r.text()));
    return (await Promise.all(ps)).reduce((t, b) => t + b.length, 0);
  };
  const run = async (n) => {
    let total = 0;
    for (let i = 0; i < n; i += C) total += await batch(C);
    return total;
  };

  await run(C * 2); // untimed warmup (also primes the connection pool)
  const t0 = performance.now();
  const total = await run(N);
  const t1 = performance.now();
  if (total === -1) console.log(total); // defeat dead-code elimination
  await stop();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
