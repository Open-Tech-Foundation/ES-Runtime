/**
 * The server. **This is the file production runs.**
 *
 * `esdev start` builds it and runs it in development, `esrun` runs the same
 * bundle in production, and nothing stands between it and the request either
 * time: a `Request` comes in, a `Response` goes out, and everything in between
 * is the platform.
 *
 * It runs under exactly what `esdev.json` grants — in development too:
 *
 * ```
 * --allow-listen=8080 --allow-env=PORT --allow-signals=SIGTERM,SIGINT
 * ```
 *
 * **No filesystem at all**, no outbound network, no subprocesses, and one
 * environment variable. A grant only added for production is a grant nobody
 * has tested.
 */
import { serve } from "runtime:http";
import { env, exit, onSignal, unmask } from "runtime:process";

import { json, toResponse } from "./http.ts";
import { Router } from "./router.ts";

const port = Number(unmask(env.PORT ?? "8080"));

// The route table. Add yours here; `path` is a `URLPattern` pathname, so
// `/things/:id` and `/files/*` work without anything to install.
const router = new Router([
  { method: "GET", path: "/", handle: index },

  // Answers the load balancer without touching anything a route depends on: a
  // health check that queries the database takes the instance down when the
  // database blinks.
  { method: "GET", path: "/healthz", handle: () => json({ ok: true }) },
]);

/** The one route this API answers. Replace it with yours. */
function index(): Response {
  return json({
    name: "{{name}}",
    runtime: "ES Runtime",
    org: "Open Tech Foundation",
    docs: "https://esrun.opentechf.org/docs",
  });
}

const server = serve({ port, hostname: "0.0.0.0" }, async (request) => {
  const url = new URL(request.url);
  const started = performance.now();
  const response = await handle(request, url);

  // One line per request, in a shape a log collector can parse. `console.log`
  // rather than a logging dependency: a line of JSON on stdout is what every
  // deployment already knows how to read.
  console.log(
    JSON.stringify({
      method: request.method,
      path: url.pathname,
      status: response.status,
      ms: Math.round((performance.now() - started) * 10) / 10,
    }),
  );
  return response;
});

async function handle(request: Request, url: URL): Promise<Response> {
  const matched = router.match(request.method, url.pathname);

  if (matched.kind === "not-found") {
    return json({ error: "Not found" }, { status: 404 });
  }
  if (matched.kind === "method-not-allowed") {
    // `Allow` is not optional on a 405: without it a client is told no and not
    // told what would work.
    return json(
      { error: "Method not allowed", allowed: matched.allowed },
      { status: 405, headers: { allow: matched.allowed.join(", ") } },
    );
  }

  try {
    return await matched.handle({ request, params: matched.params, url });
  } catch (error) {
    // The one place a failure becomes a response. `unexpected` is what tells
    // the difference between a 404 somebody asked for and a bug — and only the
    // second is worth a line in the log.
    const { response, unexpected } = toResponse(error);
    if (unexpected) {
      console.error(`unhandled ${request.method} ${url.pathname}:`, error);
    }
    return response;
  }
}

// A deployment replacing this process sends SIGTERM and then waits. Without
// this the runtime exits at once and every request still in flight is a
// connection reset in somebody's client; with it, they finish first.
let draining = false;
for (const signal of ["SIGTERM", "SIGINT"] as const) {
  onSignal(signal, () => {
    // A second signal is an operator asking again because the first appeared to
    // do nothing. Restarting the drain would reset the wait.
    if (draining) return;
    draining = true;

    console.log(JSON.stringify({ event: "draining", signal }));
    void server.stop().then(() => {
      console.log(JSON.stringify({ event: "stopped" }));
      // Explicit, because a registered signal handler is itself something the
      // runtime keeps alive: without this the process sits idle after the last
      // request instead of exiting, and the deployment waits out its grace
      // period and sends SIGKILL.
      exit(0);
    });
  });
}

const { port: bound } = await server.addr;
console.log(`listening on http://localhost:${bound}`);
