// Production + dev server: API routes, loader data, SSR pages, static assets.
//
// File-convention wiring without a filesystem scan: every route module is a
// static import (bundled), and its map key is the `/app`-rooted path the
// framework's `…RouteFromPath` helpers derive the URL from. Adding a route is
// adding an import plus one map entry; `app/` sources never ship to production.
import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { env } from "runtime:process";
import {
  createApiHandler,
  createLoaderRegistry,
  createMiddleware,
  registerRoutes,
  renderHead,
  renderRoute,
} from "@opentf/web/server";
import { pages } from "@otfw/routes";
import * as helloRoute from "./app/api/hello/route.js";
import rootLoader from "./app/loader.js";
import * as siteMiddleware from "./app/_middleware.js";

registerRoutes(pages);

// One demo API route: `app/api/hello/route.js` serves GET /api/hello (with
// auto HEAD/OPTIONS and 405 + Allow elsewhere, from the handler).
const api = createApiHandler({ "/app/api/hello/route.js": helloRoute });
// One demo loader: `app/loader.js` feeds `/` and answers `/__data.json`.
const loaders = createLoaderRegistry({ "/app/loader.js": { default: rootLoader } });
// Request middleware wraps the whole pipeline (pages, API, loaders, 404s).
const middleware = createMiddleware({ "/app/_middleware.js": siteMiddleware });

// Static assets live beside this bundle: `dist/` in production,
// `.dev/dist/` under `esdev start`.
const outDir = new URL("./", import.meta.url);
const shellFile = file(new URL("./index.html", outDir));

async function shell() {
  return shellFile.text();
}

function injectBeforeBody(html, snippet) {
  return html.includes("</body>") ? html.replace("</body>", () => `${snippet}</body>`) : html + snippet;
}

function injectMarkup(html, markup) {
  return html.replace(/(<div id="app"[^>]*>)\s*(<\/div>)/, (_m, open, close) => `${open}${markup}${close}`);
}

// The `data-otfw-hydrate` sentinel tells the client entry to adopt the server
// markup instead of rebuilding it.
function stampHydrateSentinel(html) {
  return html.replace(
    /<div id="app"([^>]*)>/,
    (m, attrs) => (/\bdata-otfw-hydrate\b/.test(attrs) ? m : `<div id="app"${attrs} data-otfw-hydrate>`),
  );
}

function renderDocument(shellHtml, { html, metadata, hydration, dataJson, path }) {
  let page = stampHydrateSentinel(injectMarkup(shellHtml, html));
  page = page.replace("</head>", () => `${renderHead(metadata ?? {}, { path })}</head>`);
  if (hydration) {
    page = injectBeforeBody(page, `<script type="application/json" id="__otfw_h">${hydration}</script>`);
  }
  if (dataJson) {
    page = injectBeforeBody(page, `<script type="application/json" id="__otfw_data">${dataJson}</script>`);
  }
  return page;
}

const server = serve({ port: Number(env.PORT ?? "3000") }, async (request) => {
  const url = new URL(request.url);
  const terminal = async (req) => {
    // API routes first; `null` falls through to loaders, pages, assets.
    const apiResponse = await api(req);
    if (apiResponse) return apiResponse;
    // Loader data endpoint: `/__data.json` for the page's own path.
    const dataResponse = await loaders.handle(req);
    if (dataResponse) return dataResponse;
    const pathname = url.pathname;
    // Static assets (bundled scripts, stylesheets, copied files). Pages win
    // over files: `/` renders through SSR below even though index.html exists.
    if (pathname !== "/" && !pathname.startsWith("/api/")) {
      const asset = file(new URL(`.${pathname}`, outDir));
      if (await asset.exists()) {
        // Module scripts are refused without a JavaScript MIME type.
        const type =
          {
            js: "text/javascript; charset=utf-8",
            mjs: "text/javascript; charset=utf-8",
            css: "text/css; charset=utf-8",
            html: "text/html; charset=utf-8",
            json: "application/json",
            svg: "image/svg+xml",
          }[pathname.split(".").pop() ?? ""] ?? "application/octet-stream";
        return new Response(asset.stream(), { headers: { "content-type": type } });
      }
    }
    // SSR: run the page's loader (when it has one), then render.
    const matched = loaders.match(pathname);
    const query = Object.fromEntries(url.searchParams);
    const { data, json } = matched ? await loaders.loadSerialized(matched, { request, query }) : {};
    const rendered = await renderRoute(pathname, matched?.params ?? null, url.search, { data });
    if (!rendered) return new Response("Not found", { status: 404 });
    return new Response(
      renderDocument(await shell(), {
        html: rendered.html,
        metadata: rendered.metadata,
        hydration: rendered.hydration,
        dataJson: json,
        path: pathname,
      }),
      { status: rendered.status, headers: { "content-type": "text/html; charset=utf-8" } },
    );
  };
  return middleware.run(request, terminal);
});

console.log(`fullstack on http://localhost:${(await server.addr).port}`);
