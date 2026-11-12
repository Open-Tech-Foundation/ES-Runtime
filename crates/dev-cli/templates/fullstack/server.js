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

// Built assets. Pages are not files: `/` and `*.html` render through SSR.
// Only a regular file is one: `/assets/` names a directory, which goes on to
// the fallback's 404 like any other path that is not a file.
async function files(request) {
  const { pathname } = new URL(request.url);
  if (pathname === "/" || pathname.endsWith(".html")) return null;
  const asset = file(new URL(`.${pathname}`, outDir));
  const isFile = await asset.stat().then((stat) => stat.isFile, () => false);
  if (!isFile) return null;
  // The runtime's type for the extension: module scripts are refused without
  // a JavaScript one.
  const type = asset.type || "application/octet-stream";
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
