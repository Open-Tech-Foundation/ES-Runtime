// Static-file HTTP server: the same shape as helloserver.js, but each response
// is a 64 KiB file read from disk rather than a constant string. Driven by an
// external load generator (rps.sh), so the number is the server alone.
//
// This is the path a static asset takes, and it is the one place the runtimes
// differ structurally: Bun and Deno can hand a file handle to the kernel and let
// `sendfile` move the bytes without them ever entering the process, while a
// runtime that reads into a buffer and writes it back pays a copy per request.
// The `fsread` rows measure reading a file; this measures reading it *and*
// getting it onto a socket, which is where that difference shows up.
//
// The file is created on startup so the run needs no fixture checked in, and it
// is served from page cache — as it would be in production, where a hot asset
// is not fetched from the platter on every request.
const PORT = Number(await benchEnv("BENCH_PORT")) || 3000;
const FILE = (await benchEnv("BENCH_STATIC_FILE")) || "bench_static_asset.bin";
const SIZE = 65536;

async function benchEnv(name) {
  if (typeof Deno !== "undefined") return Deno.env.get(name);
  if (typeof process !== "undefined" && process.env) return process.env[name];
  const { env } = await import("runtime:process");
  return env[name];
}

const NODE_LIKE =
  typeof Bun !== "undefined" ||
  (typeof process !== "undefined" && process.versions && process.versions.node);

const payload = "x".repeat(SIZE);

if (typeof Deno !== "undefined") {
  await Deno.writeFile(FILE, new TextEncoder().encode(payload));
  Deno.serve({ hostname: "127.0.0.1", port: PORT, onListen() {} }, async () => {
    // Deno streams a file handle straight to the socket.
    const f = await Deno.open(FILE, { read: true });
    return new Response(f.readable, { headers: { "content-type": "text/plain" } });
  });
} else if (typeof Bun !== "undefined") {
  await Bun.write(FILE, payload);
  Bun.serve({
    hostname: "127.0.0.1",
    port: PORT,
    // Handing Bun.file() to Response is the sendfile path.
    fetch: () => new Response(Bun.file(FILE)),
  });
} else if (NODE_LIKE) {
  const fsp = await import("node:fs/promises");
  const fss = await import("node:fs");
  const http = await import("node:http");
  await fsp.writeFile(FILE, payload);
  http
    .createServer((_req, res) => {
      res.setHeader("content-type", "text/plain");
      // createReadStream().pipe() is Node's idiomatic static-file path.
      fss.createReadStream(FILE).pipe(res);
    })
    .listen(PORT, "127.0.0.1");
} else {
  const fs = await import("runtime:fs");
  const { serve } = await import("runtime:http");
  await fs.write(FILE, payload);
  serve({ hostname: "127.0.0.1", port: PORT }, async () => {
    const bytes = await fs.file(FILE).arrayBuffer();
    return new Response(bytes, { headers: { "content-type": "text/plain" } });
  });
}
