/**
 * The static build: every route, rendered once, written as a file.
 *
 * It runs because `esdev.json` says `"then": "run"` — the bundle is built and
 * then executed, and what it writes is the build's real output. `esdev` does
 * not know what a static site is; this file does.
 */
import { copy, mkdir, readDir, write } from "runtime:fs";
import { join } from "runtime:path";
import { here } from "./document.ts";
import { render } from "./render.tsx";
import { routes } from "./app/routes.ts";

/** Written beside the server build rather than over it: `dist/index.html` is
 *  the *template* the server splices into, and a rendered page is not that. */
const out = join(here, "static");

for (const route of routes) {
  const data = await route.loader();
  const html = await new Response(await render(route, data)).text();
  const target = route.path === "/" ? "index.html" : `${route.path.slice(1)}/index.html`;
  if (target.includes("/")) {
    await mkdir(join(out, target.slice(0, target.lastIndexOf("/"))), { recursive: true });
  } else {
    await mkdir(out, { recursive: true });
  }
  await write(join(out, target), html);
  console.log(`  static/${target}`);
}

// The pages reference /assets/…, so the directory has to come with them: what
// gets deployed is `dist/static`, whole.
await mkdir(join(out, "assets"), { recursive: true });
for (const entry of await readDir(join(here, "assets"))) {
  if (entry.isFile) {
    await copy(join(here, "assets", entry.name), join(out, "assets", entry.name));
  }
}
