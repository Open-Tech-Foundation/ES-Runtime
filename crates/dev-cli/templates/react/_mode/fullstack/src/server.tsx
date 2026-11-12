import { serve } from "runtime:http";
import { file } from "runtime:fs";
import { join } from "runtime:path";
import { env, unmask } from "runtime:process";
import { here, renderPage } from "./render.tsx";

const port = Number(unmask(env.PORT ?? "8080"));

const server = serve({ port }, async (request) => {
  const { pathname } = new URL(request.url);

  // What the build wrote to dist/assets. Only a regular file is one: `/assets/`
  // names a directory, and is a 404 like any other path that is not a file.
  if (pathname.startsWith("/assets/") && !pathname.includes("..")) {
    const asset = file(join(here, pathname.slice(1)));
    const isFile = await asset.stat().then((stat) => stat.isFile, () => false);
    if (!isFile) return new Response("Not Found", { status: 404 });
    // The runtime's type for the extension.
    const type = asset.type || "application/octet-stream";
    return new Response(asset.stream(), { headers: { "content-type": type } });
  }

  return new Response(await renderPage(), {
    headers: { "content-type": "text/html; charset=utf-8" },
  });
});

console.log(`listening on http://localhost:${(await server.addr).port}`);
