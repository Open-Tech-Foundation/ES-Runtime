// Same hello-world req/s shape as helloserver.js, but served through Hono — a
// real, third-party web framework — to show esrun runs unmodified npm ESM
// packages, not just its own server. This is the "framework" counterpart to the
// Bun framework charts (e.g. their Express number). Hono is Web-standard: its
// `app.fetch(request) -> Response` handler plugs straight into every runtime's
// native server. Node has no Web-standard server, so it uses Hono's official
// @hono/node-server adapter. Run by rps.sh via SERVER=scripts/hono.js.
//
// Install once (in bench/):  bun add hono @hono/node-server
import { Hono } from "hono";

const app = new Hono();
app.get("/", (c) => c.text("Hello, World!"));

const PORT = 3000;

if (typeof Deno !== "undefined") {
  Deno.serve({ hostname: "127.0.0.1", port: PORT, onListen() {} }, app.fetch);
} else if (typeof Bun !== "undefined") {
  Bun.serve({ hostname: "127.0.0.1", port: PORT, fetch: app.fetch });
} else if (typeof process !== "undefined" && process.versions && process.versions.node) {
  const { serve } = await import("@hono/node-server");
  serve({ fetch: app.fetch, hostname: "127.0.0.1", port: PORT });
} else {
  const { serve } = await import("runtime:http");
  serve({ hostname: "127.0.0.1", port: PORT }, app.fetch);
}
