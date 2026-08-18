/**
 * The static build: every route, rendered once, written as a file.
 *
 * It runs because `esdev.json` says `"then": "run"` on this target — the bundle
 * is built and then executed, and what it writes is the build's real output.
 * esdev does not know what a static site is; this file does.
 *
 * What comes out is a directory any static host can serve, with no server at
 * all — and because it goes through the same `render` the server uses, a page
 * cannot come out one way here and another way live.
 */
import { copy, mkdir, readDir, write } from "runtime:fs";
import { join } from "runtime:path";

import { here } from "./document.ts";
import { render } from "./render.tsx";
import { staticPaths } from "./paths.ts";

/**
 * Written beside the server build rather than over it.
 *
 * `dist/index.html` is the *template* the server splices into — it still has
 * its markers, and its head is the default one. A rendered page is not that,
 * and overwriting it would leave a server that serves finished pages as if they
 * were templates.
 */
const out = join(here, "static");

// The origin these pages will be served from is not knowable at build time, and
// nothing in the output depends on it: every URL the app emits is rooted. This
// is here because `Request` requires an absolute URL, and nothing more.
const ORIGIN = "http://prerender.local";

let written = 0;

for (const path of await staticPaths()) {
  const rendered = await render(new Request(new URL(path, ORIGIN)), "");

  if (rendered instanceof Response) {
    // A redirect has no file to be. A static host expresses one with its own
    // configuration, which is not something this build can write.
    console.warn(`  skipped ${path} — it redirects to ${rendered.headers.get("location")}`);
    continue;
  }
  if (rendered.status !== 200) {
    // A page that does not render is a broken deployment, and finding out here
    // costs a failed build rather than a 404 somebody reports later.
    throw new Error(`${path} rendered ${rendered.status}; the static build expects 200`);
  }

  // **`allReady`, not the stream.** Reading the body would give the shell as
  // soon as it is ready, which is right for a response over the wire and wrong
  // for a file: anything inside a `<Suspense>` boundary would be missing, and
  // the page would be silently incomplete.
  await rendered.allReady;
  const html = await new Response(rendered.body).text();

  // `/about` becomes `about/index.html`, so a host serves it at the same
  // URL the router uses. `/` is the one that is already an index.
  const file = path === "/" ? "index.html" : `${path.replace(/^\//, "")}/index.html`;
  await mkdir(join(out, dirOf(file)), { recursive: true });
  await write(join(out, file), html);
  console.log(`  static/${file}`);
  written++;
}

// The pages reference /assets/…, so the directory has to travel with them. What
// gets deployed is `dist/static`, whole.
await mkdir(join(out, "assets"), { recursive: true });
for (const entry of await readDir(join(here, "assets"))) {
  if (entry.isFile) {
    await copy(join(here, "assets", entry.name), join(out, "assets", entry.name));
  }
}

// Most static hosts serve `404.html` for anything they cannot find. Rendering
// it through the app is what keeps a missing page looking like the rest of the
// site instead of like the host's default.
const missing = await render(new Request(new URL("/404", ORIGIN)), "");
if (!(missing instanceof Response)) {
  await missing.allReady;
  await write(join(out, "404.html"), await new Response(missing.body).text());
  console.log("  static/404.html");
}

console.log(`prerendered ${written} page${written === 1 ? "" : "s"} to dist/static`);

/** The directory part of a relative path, or `.` when there is none. */
function dirOf(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "." : path.slice(0, slash);
}
