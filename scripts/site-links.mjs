#!/usr/bin/env node

/* Check site links against the routes and anchors produced by the SSG build.
 * Run from website/ after `pnpm run build`; keeping this outside the website
 * package makes it usable from CI without adding another dependency. */

import { readFile, readdir } from "node:fs/promises";
import { basename, dirname, extname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const appRoot = resolve(repoRoot, "website/app");
const distRoot = resolve(repoRoot, "website/dist");
const sourceFiles = [];

async function walk(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) await walk(path);
    else if ([".mdx", ".jsx", ".js"].includes(extname(entry.name))) sourceFiles.push(path);
  }
}

function routeForSource(path) {
  if (path === resolve(appRoot, "page.jsx")) return "/";
  const page = path.endsWith("/page.mdx") ? dirname(path) : null;
  if (!page) return null;
  const rel = relative(appRoot, page).replaceAll("\\", "/");
  return rel ? `/${rel}` : "/";
}

function routeForDist(path) {
  const rel = relative(distRoot, dirname(path)).replaceAll("\\", "/");
  return rel ? `/${rel}` : "/";
}

function normalizeRoute(path) {
  if (path.length > 1 && path.endsWith("/")) return path.slice(0, -1);
  return path;
}

function isStaticAsset(path) {
  return (
    path === "/og.png" ||
    path.startsWith("/assets/") ||
    path.startsWith("/app/") ||
    path.startsWith("/img/")
  );
}

await walk(appRoot);
sourceFiles.push(resolve(repoRoot, "website/index.html"));

const distFiles = [];
async function walkDist(dir) {
  for (const entry of await readdir(dir, { withFileTypes: true })) {
    const path = resolve(dir, entry.name);
    if (entry.isDirectory()) await walkDist(path);
    else if (basename(path) === "index.html") distFiles.push(path);
  }
}

await walkDist(distRoot);

const routes = new Map();
for (const path of distFiles) routes.set(normalizeRoute(routeForDist(path)), path);

// These are intentionally kept as valid legacy routes while old bookmarks
// continue to redirect through website/app/routeGuard.js.
const redirects = new Set(["/docs/typescript", "/docs/esdev"]);
const links = [];
for (const source of sourceFiles) {
  const text = await readFile(source, "utf8");
  const sourceRoute = routeForSource(source) ?? (source.endsWith("website/index.html") ? "/" : null);
  const addLink = (raw) => {
    const [pathPart, fragment] = raw.split("#", 2);
    const path = normalizeRoute(pathPart.split("?", 1)[0]);
    if (!path || isStaticAsset(path)) return;
    links.push({ source, raw, path, fragment });
  };

  for (const match of text.matchAll(/\]\((\/[^)\s]+)\)|(?:href|path)=["'](\/[^"']+)["']/g)) {
    addLink(match[1] ?? match[2]);
  }

  if (sourceRoute) {
    for (const match of text.matchAll(/\]\(#([^)\s]+)\)|(?:href|path)=["'](#[-\w]+)["']/g)) {
      addLink(`${sourceRoute}#${match[1] ?? match[2]}`);
    }
  }
}

const errors = [];
for (const link of links) {
  if (!routes.has(link.path) && !redirects.has(link.path)) {
    errors.push(`${relative(repoRoot, link.source)} -> ${link.raw} (route not found)`);
    continue;
  }
  if (!link.fragment || redirects.has(link.path)) continue;
  const html = await readFile(routes.get(link.path), "utf8");
  if (!html.includes(`id="${link.fragment}"`)) {
    errors.push(`${relative(repoRoot, link.source)} -> ${link.raw} (heading not found)`);
  }
}

if (errors.length) {
  console.error(`Site link audit failed with ${errors.length} error(s):`);
  for (const error of errors) console.error(`- ${error}`);
  process.exitCode = 1;
} else {
  console.log(`Site link audit passed: ${links.length} internal links checked.`);
}
