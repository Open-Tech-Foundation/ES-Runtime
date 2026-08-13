/**
 * The server. **This is the file production runs** — `esdev start` builds it
 * and runs it in development, `esrun` runs the same bundle in production, and
 * nothing stands between it and the request either time.
 */
import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { join } from "runtime:path";
import { here, document } from "./document.ts";
import { render } from "./render.tsx";
import { match } from "./app/routes.ts";

const port = Number(globalThis.process?.env?.PORT ?? 8080);

/** Content types for what the build writes. */
const TYPES: Record<string, string> = {
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  svg: "image/svg+xml",
  png: "image/png",
  ico: "image/x-icon",
  woff2: "font/woff2",
};

/**
 * Everything the build hashed lives under `/assets`, so it is safe to cache for
 * ever: a changed file gets a changed name. The path is rejected rather than
 * cleaned if it tries to climb — `..` in a URL is never a real request.
 */
async function asset(pathname: string): Promise<Response> {
  const name = pathname.slice("/assets/".length);
  if (!name || name.includes("..") || name.includes("\\")) {
    return new Response("not found", { status: 404 });
  }
  const handle = file(join(here, "assets", name));
  if (!(await handle.exists())) {
    return new Response("not found", { status: 404 });
  }
  const type = TYPES[name.split(".").pop() ?? ""] ?? "application/octet-stream";
  return new Response(handle.stream(), {
    headers: {
      "content-type": type,
      "cache-control": "public, max-age=31536000, immutable",
    },
  });
}

const server = serve({ port, hostname: "0.0.0.0" }, async (request) => {
  const url = new URL(request.url);

  if (url.pathname.startsWith("/assets/")) return asset(url.pathname);

  // An API route, to show the shape: this is an ordinary handler on an
  // ordinary web Request, and everything the platform gives you works here.
  if (url.pathname === "/api/time") {
    return Response.json({ now: new Date().toISOString() });
  }

  const route = match(url.pathname);
  if (!route) {
    return new Response(document.beforeApp + "<h1>404</h1>" + document.afterApp + document.afterData, {
      status: 404,
      headers: { "content-type": "text/html; charset=utf-8" },
    });
  }

  const data = await route.loader();
  return new Response(await render(route, data), {
    headers: { "content-type": "text/html; charset=utf-8" },
  });
});

const { port: bound } = await server.addr;
console.log(`listening on http://localhost:${bound}`);
