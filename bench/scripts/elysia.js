// Same hello-world req/s shape as hono.js, but served through Elysia — the
// second real, third-party web framework in the rps comparison. Elysia is
// Web-standard (`app.fetch(request) -> Response`, WinterTC-compliant), so it
// plugs straight into every runtime's native server; Node shares Hono's
// `@hono/node-server` glue (see the branch below). Measured from the esdev
// bundle (`dist/elysia.bundle.js`, built by gen-bench-data.sh), because a
// transitive dependency is CommonJS and esrun is ESM-only.
//
// Install once (in bench/):  pnpm add elysia
import { Elysia } from "elysia";

// Port from BENCH_PORT (rps.sh picks a free one per run); see helloserver.js.
async function benchPort() {
  if (typeof Deno !== "undefined") return Deno.env.get("BENCH_PORT");
  if (typeof process !== "undefined" && process.env) return process.env.BENCH_PORT;
  const { env } = await import("runtime:process");
  return env.BENCH_PORT;
}

const PORT = Number(await benchPort()) || 3000;

if (typeof Deno !== "undefined") {
  const app = new Elysia().get("/", () => "Hello, World!");
  Deno.serve({ hostname: "127.0.0.1", port: PORT, onListen() {} }, app.fetch);
} else if (typeof Bun !== "undefined") {
  const app = new Elysia().get("/", () => "Hello, World!");
  app.listen({ hostname: "127.0.0.1", port: PORT });
} else if (typeof process !== "undefined" && process.versions && process.versions.node) {
  // The same glue Hono uses: Elysia's official Node adapter is srvx-based
  // CJS, which does not survive the esdev bundle this section measures, so
  // both frameworks share one Node server and the delta is handling, not glue.
  const app = new Elysia().get("/", () => "Hello, World!");
  const { serve } = await import("@hono/node-server");
  serve({ fetch: app.fetch, hostname: "127.0.0.1", port: PORT });
} else {
  const app = new Elysia().get("/", () => "Hello, World!");
  const { serve } = await import("runtime:http");
  serve({ hostname: "127.0.0.1", port: PORT }, app.fetch);
}
