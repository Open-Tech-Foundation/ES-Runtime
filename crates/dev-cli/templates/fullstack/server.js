// Production + dev server. `@afw/server` is generated from `app/`: it runs the
// middleware around API routes, server functions, loader data and
// server-rendered pages, so a new route, loader or middleware needs no edit
// here. What stays yours is how static files are served.
import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { env } from "runtime:process";
import { createHandler, createPageRenderer } from "@afw/server";

// Beside this bundle: `dist/` in production, `.dev/dist/` under `esdev start`.
const outDir = new URL("./", import.meta.url);

// Module scripts are refused without a JavaScript MIME type.
const TYPES = {
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  json: "application/json",
  svg: "image/svg+xml",
  map: "application/json",
};

// Built assets. Pages are not files: `/` and `*.html` render through SSR.
async function files(request) {
  const { pathname } = new URL(request.url);
  if (pathname === "/" || pathname.endsWith(".html")) return null;
  const asset = file(new URL(`.${pathname}`, outDir));
  if (!(await asset.exists())) return null;
  const type = TYPES[pathname.split(".").pop()] ?? "application/octet-stream";
  return new Response(asset.stream(), { headers: { "content-type": type } });
}

const shell = await file(new URL("./index.html", outDir)).text();

const server = serve(
  { port: Number(env.PORT ?? "3000") },
  createHandler({
    files,
    renderPage: createPageRenderer({ shell }),
    fallback: () => new Response("Not found", { status: 404 }),
  }),
);

console.log(`fullstack on http://localhost:${(await server.addr).port}`);
