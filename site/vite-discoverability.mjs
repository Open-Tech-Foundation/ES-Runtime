// Vite plugin: regenerate public/sitemap.xml and public/llms.txt from the routes
// on disk at every build and dev-server start, so they never drift from the
// pages. Writes into public/ (which Vite serves in dev and copies into dist on
// build); both files are gitignored. The generated artifacts are produced by
// scripts/discoverability.mjs (also runnable on its own).
//
// (Our web framework doesn't do route-aware SSG yet — this plugin fills that gap
// for the two discoverability files.)
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { buildDiscoverability } from "./scripts/discoverability.mjs";

const PUBLIC_DIR = fileURLToPath(new URL("./public", import.meta.url));

export default function discoverability() {
  const write = (warn) => {
    const { routes, files, missing, stale } = buildDiscoverability();
    mkdirSync(PUBLIC_DIR, { recursive: true });
    for (const [name, content] of Object.entries(files)) {
      writeFileSync(path.join(PUBLIC_DIR, name), content);
    }
    if (missing.length) warn(`no llms.txt metadata for: ${missing.join(", ")} (add to src/site-meta.js)`);
    if (stale.length) warn(`src/site-meta.js lists routes that no longer exist: ${stale.join(", ")}`);
    return routes.length;
  };

  return {
    name: "esrun-discoverability",
    // Runs in both `vite build` and `vite dev`.
    buildStart() {
      const n = write((m) => this.warn(m));
      this.info?.(`discoverability: regenerated sitemap.xml, llms.txt, llms-full.txt (${n} routes)`);
    },
  };
}
