// Generates sitemap.xml and llms.txt from the routes on disk + src/site-meta.js.
//
// Routes are discovered from app/**/page.{jsx,tsx,js,ts} (the same files the app
// router globs at runtime), so the sitemap is always complete. llms.txt pairs
// each route with its curated description from site-meta; a route with no
// description is reported (not silently dropped).
//
// Used by vite-discoverability.mjs at build/dev start, and runnable directly:
//   node scripts/discoverability.mjs        # writes into public/
import { globSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  SITE_URL,
  SITE_NAME,
  TAGLINE,
  SUMMARY,
  SECTION_ORDER,
  PAGES,
  FOOTER,
  FULL_DOC,
  SITEMAP_EXTRA,
  SITEMAP_EXCLUDE,
} from "../src/site-meta.js";

const APP_DIR = fileURLToPath(new URL("../app", import.meta.url));
const PUBLIC_DIR = fileURLToPath(new URL("../public", import.meta.url));
const REPO_ROOT = fileURLToPath(new URL("../../", import.meta.url));

// All routes, derived from page files: app/docs/glob/page.jsx -> /docs/glob,
// app/page.jsx -> /.
export function discoverRoutes(appDir = APP_DIR) {
  const files = globSync("**/page.*", { cwd: appDir }).filter((f) =>
    /(^|\/)page\.(jsx|tsx|js|ts)$/.test(f),
  );
  const routes = files.map((f) => {
    const trimmed = f.replace(/(^|\/)page\.(jsx|tsx|js|ts)$/, "");
    return trimmed === "" ? "/" : "/" + trimmed;
  });
  return [...new Set(routes)].filter((r) => !SITEMAP_EXCLUDE.has(r)).sort();
}

function priorityFor(route) {
  if (route === "/") return "1.0";
  const depth = route.split("/").filter(Boolean).length;
  return depth <= 1 ? "0.9" : depth === 2 ? "0.7" : "0.6";
}

export function renderSitemap(routes) {
  const entries = [
    { path: "/", priority: "1.0", weekly: true },
    ...routes
      .filter((r) => r !== "/")
      .map((r) => ({ path: r, priority: priorityFor(r), weekly: /^\/(docs|api)$/.test(r) })),
    ...SITEMAP_EXTRA,
  ];
  const urls = entries.map((e) => {
    const loc = `${SITE_URL}${e.path === "/" ? "/" : e.path}`;
    const cf = e.weekly ? "    <changefreq>weekly</changefreq>\n" : "";
    return `  <url>\n    <loc>${loc}</loc>\n${cf}    <priority>${e.priority || "0.6"}</priority>\n  </url>`;
  });
  return `<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n${urls.join("\n")}\n</urlset>\n`;
}

export function renderLlmsTxt(routes) {
  const present = new Set(routes);
  // A discovered route with no metadata, and metadata for a route that's gone.
  const missing = routes.filter((r) => r !== "/" && !PAGES[r]);
  const stale = Object.keys(PAGES).filter((r) => !present.has(r));

  let out = `# ${SITE_NAME}\n\n> ${TAGLINE}\n\n${SUMMARY}\n`;
  for (const section of SECTION_ORDER) {
    // PAGES insertion order = curated reading order; include only live routes.
    const routesInSection = Object.keys(PAGES).filter(
      (r) => PAGES[r].section === section && present.has(r),
    );
    if (routesInSection.length === 0) continue;
    out += `\n## ${section}\n\n`;
    for (const r of routesInSection) {
      const { title, description } = PAGES[r];
      out += `- [${title}](${SITE_URL}${r}): ${description}\n`;
    }
  }
  for (const f of FOOTER) out += `\n## ${f.section}\n\n${f.body}\n`;
  return { content: out, missing, stale };
}

// Inlines the canonical repo docs (README + docs/API.md) as one markdown file.
export function renderLlmsFull(repoRoot = REPO_ROOT) {
  const header = `# ${SITE_NAME} — full documentation\n\n> ${FULL_DOC.tagline}\n\n_${FULL_DOC.note}_\n\n---\n`;
  const parts = FULL_DOC.sources.map((rel) =>
    readFileSync(path.join(repoRoot, rel), "utf8").trim(),
  );
  return `${header}\n${parts.join("\n\n---\n\n")}\n`;
}

// Builds all three files; returns their contents plus any warnings.
export function buildDiscoverability(appDir = APP_DIR) {
  const routes = discoverRoutes(appDir);
  const sitemap = renderSitemap(routes);
  const { content: llms, missing, stale } = renderLlmsTxt(routes);
  return {
    routes,
    files: {
      "sitemap.xml": sitemap,
      "llms.txt": llms,
      "llms-full.txt": renderLlmsFull(),
    },
    missing,
    stale,
  };
}

// Run directly: write into public/ and report.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const { routes, files, missing, stale } = buildDiscoverability();
  mkdirSync(PUBLIC_DIR, { recursive: true });
  for (const [name, content] of Object.entries(files)) {
    writeFileSync(path.join(PUBLIC_DIR, name), content);
  }
  if (missing.length) console.warn(`⚠ no llms.txt metadata (add to src/site-meta.js): ${missing.join(", ")}`);
  if (stale.length) console.warn(`⚠ site-meta.js entries for missing routes: ${stale.join(", ")}`);
  console.log(`Wrote ${Object.keys(files).join(", ")} to public/ (${routes.length} routes).`);
}
