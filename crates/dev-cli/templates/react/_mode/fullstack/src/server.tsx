/**
 * The server. **This is the file production runs.**
 *
 * `esdev start` builds it and runs it in development, `esrun` runs the same
 * bundle in production, and nothing stands between it and the request either
 * time. There is no framework here to learn: a `Request` comes in, a `Response`
 * goes out, and everything in between is the platform.
 *
 * It runs under exactly the permissions in `esdev.json` — in development too.
 * A grant that is only added for production is a grant nobody has tested.
 */
import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { join } from "runtime:path";
import { env, exit, onSignal, unmask } from "runtime:process";

import { here } from "./document.ts";
import { render } from "./render.tsx";
import { ASSET_PREFIX, cacheControl, contentType, isAssetName } from "./http/assets.ts";
import { nonce, securityHeaders } from "./http/headers.ts";

// `unmask` because an env entry can arrive as a `Secret` — a wrapper that keeps
// a value out of logs and stack traces. PORT is not one, and `unmask` on a
// plain string returns it unchanged, so this is right either way.
//
// Reading it at all needs `--allow-env=PORT`, which `esdev.json` grants. That
// is the whole of this program's access to the environment.
const port = Number(unmask(env.PORT ?? "8080"));

// Replaced with a literal by the build, so this is a constant and the branches
// on it are eliminated. `esdev start` defines it as "development"; `esdev build`
// as "production". Read once here rather than deep inside `src/http/`, which has
// no imports on purpose and must stay runnable unbundled by `esdev test`.
const DEVELOPMENT = process.env.NODE_ENV !== "production";

/**
 * A file from `dist/assets`.
 *
 * The name is checked rather than cleaned ([`isAssetName`]), and the file is
 * streamed rather than read — a font is not worth holding in memory to hand
 * over one chunk at a time.
 */
async function asset(pathname: string): Promise<Response> {
  const name = pathname.slice(ASSET_PREFIX.length);
  if (!isAssetName(name)) {
    return new Response("Not Found", { status: 404 });
  }

  const handle = file(join(here, "assets", name));
  const stat = await handle.stat().catch(() => undefined);
  if (!stat?.isFile) {
    return new Response("Not Found", { status: 404 });
  }

  return new Response(handle.stream(), {
    headers: {
      "content-type": contentType(name),
      "content-length": String(stat.size),
      "cache-control": cacheControl(DEVELOPMENT),
      "x-content-type-options": "nosniff",
    },
  });
}

/** A document response, rendered by the app. */
async function page(request: Request): Promise<Response> {
  const scriptNonce = nonce();
  const rendered = await render(request, scriptNonce);

  // A loader redirected. It is already a complete answer.
  if (rendered instanceof Response) {
    return rendered;
  }

  return new Response(rendered.body, {
    status: rendered.status,
    headers: {
      "content-type": "text/html; charset=utf-8",
      // The document is rendered per request and is never the same twice — a
      // shared cache holding it would serve one visitor's page to another.
      "cache-control": "no-store",
      ...securityHeaders(scriptNonce, DEVELOPMENT),
    },
  });
}

const server = serve({ port, hostname: "0.0.0.0" }, async (request) => {
  const url = new URL(request.url);
  const started = performance.now();

  const response = await handle(request, url).catch((error: unknown) => {
    // The last line of defence. Something threw where nothing should, and the
    // one thing that must not happen is the connection hanging.
    console.error(`unhandled ${request.method} ${url.pathname}:`, error);
    return new Response("Internal Server Error", {
      status: 500,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  });

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
  // Nothing here is written to, so anything else is answered before it reaches
  // a route and renders a page the browser will discard.
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("Method Not Allowed", {
      status: 405,
      headers: { allow: "GET, HEAD" },
    });
  }

  // Before the router, because an asset is not a page and a miss here should be
  // a 404 rather than the app's 404 *page* with a stylesheet's URL in it.
  if (url.pathname.startsWith(ASSET_PREFIX)) {
    return asset(url.pathname);
  }

  // Answers the load balancer without touching the router or the data source:
  // a health check that renders a page reports on the renderer, and a health
  // check that queries a database fails the instance when the database blinks.
  if (url.pathname === "/healthz") {
    return new Response("ok", {
      headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
    });
  }

  // An API route, to show the shape. An ordinary handler on an ordinary
  // `Request` — everything the platform gives you works here.
  if (url.pathname === "/api/time") {
    return Response.json(
      { now: new Date().toISOString() },
      { headers: { "cache-control": "no-store" } },
    );
  }

  return page(request);
}

// A deployment replacing this process sends SIGTERM and then waits. Without
// this the runtime exits at once and every request still in flight is a
// connection reset in somebody's browser; with it, they finish first.
//
// `stop()` closes the listener, so the load balancer stops sending new work
// while the requests already accepted run to completion.
let draining = false;
for (const signal of ["SIGTERM", "SIGINT"] as const) {
  onSignal(signal, () => {
    // A second signal is an operator asking again because the first appeared to
    // do nothing. Restarting the drain would reset the wait; this leaves the
    // one already running alone.
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
