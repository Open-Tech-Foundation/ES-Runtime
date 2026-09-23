import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { join } from "runtime:path";
import { env, unmask } from "runtime:process";
import { here, renderPage } from "./render.tsx";

const port = Number(unmask(env.PORT ?? "8080"));

const types: Record<string, string> = {
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  svg: "image/svg+xml",
};

const server = serve({ port }, async (request) => {
  const { pathname } = new URL(request.url);

  // What the build wrote to dist/assets.
  if (pathname.startsWith("/assets/") && !pathname.includes("..")) {
    const asset = file(join(here, pathname.slice(1)));
    const type = types[pathname.split(".").pop() ?? ""] ?? "application/octet-stream";
    return asset
      .stat()
      .then(() => new Response(asset.stream(), { headers: { "content-type": type } }))
      .catch(() => new Response("Not Found", { status: 404 }));
  }

  return new Response(await renderPage(), {
    headers: { "content-type": "text/html; charset=utf-8" },
  });
});

console.log(`listening on http://localhost:${(await server.addr).port}`);
