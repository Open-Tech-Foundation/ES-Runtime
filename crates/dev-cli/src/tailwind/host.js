// esdev: Tailwind CSS, compiled by the project's own `tailwindcss`.
//
// Generated into a run of its own by `crate::tailwind`, which replaces
// __ESDEV_TAILWIND__ with the `file:` URL of the package's compiler. What this
// program does is the part of Tailwind's own integrations (@tailwindcss/vite,
// @tailwindcss/postcss) that is not the scanner: load stylesheets and plugins
// for the compiler, and hold one compiler per stylesheet between builds.
//
// Two calls per stylesheet, both through the one `transform` hook:
//
//   1. compile — no `candidates` on the context. Returns where to look for
//      class names (`sources`, `root`) and whether the sheet has utilities to
//      generate at all. The scanning is esdev's, in Rust.
//   2. build — `candidates` on the context. Returns the CSS.

import { host } from "runtime:build";
import { exists, file } from "runtime:fs";
import { dirname, fromFileURL, isAbsolute, join, toFileURL } from "runtime:path";

const { compile } = await import(__ESDEV_TAILWIND__);

// `Features.Utilities` in Tailwind's own enum: the sheet has `@tailwind
// utilities` (which `@import "tailwindcss"` brings), so it generates classes
// from candidates. A sheet without it — a CSS Module using `@apply` against a
// `@reference` — needs no scan at all.
const UTILITIES = 16;

// One compiler per stylesheet, kept while its source is unchanged. Compiling
// parses the theme and every stylesheet it imports; `build()` after that only
// generates what is new, so the dev loop pays for the parse once per edit to
// the stylesheet rather than once per save anywhere.
const compiled = new Map();

function compilerFor(id, css) {
  let entry = compiled.get(id);
  if (entry === undefined || entry.css !== css) {
    const read = new Set();
    entry = {
      css,
      read,
      compiler: compile(css, {
        base: dirname(id),
        from: id,
        loadStylesheet: (request, base) => loadStylesheet(request, base, read),
        loadModule: (request, base) => loadModule(request, base, read),
      }),
    };
    compiled.set(id, entry);
  }
  return entry;
}

async function loadStylesheet(request, base, read) {
  const path = await resolveStylesheet(request, base);
  read.add(path);
  return { path, base: dirname(path), content: await file(path).text() };
}

async function loadModule(request, base, read) {
  // A plugin in the project is a path; one from a package is found the way
  // any import of it would be, through the module loader.
  //
  // `join` does not restart at an absolute segment, and esdev has already made
  // a relative `@plugin` absolute against the file that wrote it.
  const path = isAbsolute(request)
    ? request
    : request.startsWith(".")
      ? join(base, request)
      : fromFileURL(import.meta.resolve(request));
  read.add(path);
  const module = await import(toFileURL(path).href);
  return { path, base: dirname(path), module: module.default ?? module };
}

// --- stylesheets in packages ------------------------------------------------
//
// What `@import "tailwindcss"` means: the package's *stylesheet*, which its
// `exports` name under the `style` condition. Tailwind's own integrations
// resolve with exactly that condition, and so does this — the module loader
// cannot, because for a JavaScript import `tailwindcss` is its compiler.

async function resolveStylesheet(request, base) {
  if (isAbsolute(request)) return request;
  if (request.startsWith(".")) return join(base, request);

  const { name, subpath } = splitPackage(request);
  for (let dir = base; ; ) {
    const root = join(dir, "node_modules", name);
    const manifestPath = join(root, "package.json");
    if (await exists(manifestPath)) {
      const manifest = JSON.parse(await file(manifestPath).text());
      const target = stylesheetOf(manifest, subpath);
      if (target === null) {
        throw new Error(`${request}: ${name} does not export a stylesheet at "${subpath}"`);
      }
      return join(root, target);
    }
    const parent = dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  throw new Error(`cannot find the stylesheet "${request}" — is ${name} installed?`);
}

function splitPackage(request) {
  const parts = request.split("/");
  const count = request.startsWith("@") ? 2 : 1;
  const name = parts.slice(0, count).join("/");
  const rest = parts.slice(count).join("/");
  return { name, subpath: rest === "" ? "." : `./${rest}` };
}

// The file a package's manifest names for `subpath`, or null.
function stylesheetOf(manifest, subpath) {
  const exportsField = manifest.exports;
  if (exportsField !== undefined && exportsField !== null) {
    return condition(exportEntry(exportsField, subpath));
  }
  if (subpath === ".") return manifest.style ?? "index.css";
  return subpath;
}

function exportEntry(exportsField, subpath) {
  const keyed =
    typeof exportsField === "object" &&
    !Array.isArray(exportsField) &&
    Object.keys(exportsField).some((key) => key.startsWith("."));
  if (!keyed) return subpath === "." ? exportsField : null;
  if (subpath in exportsField) return exportsField[subpath];
  for (const [key, value] of Object.entries(exportsField)) {
    const star = key.indexOf("*");
    if (star === -1) continue;
    const before = key.slice(0, star);
    const after = key.slice(star + 1);
    if (subpath.startsWith(before) && subpath.endsWith(after)) {
      const captured = subpath.slice(before.length, subpath.length - after.length);
      return substitute(value, captured);
    }
  }
  return null;
}

function substitute(value, captured) {
  if (typeof value === "string") return value.replaceAll("*", captured);
  if (Array.isArray(value)) return value.map((item) => substitute(item, captured));
  if (value !== null && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, substitute(item, captured)]),
    );
  }
  return value;
}

// Conditions in the manifest's own key order, as every resolver does: the
// first key that is `style` or `default` wins.
function condition(entry) {
  if (entry === null || entry === undefined) return null;
  if (typeof entry === "string") return entry;
  if (Array.isArray(entry)) {
    for (const item of entry) {
      const found = condition(item);
      if (found !== null) return found;
    }
    return null;
  }
  for (const [key, value] of Object.entries(entry)) {
    if (key === "style" || key === "default") {
      const found = condition(value);
      if (found !== null) return found;
    }
  }
  return null;
}

// --- the pass ----------------------------------------------------------------

const tailwind = {
  name: "esdev:tailwind",
  transform: {
    filter: { id: /\.css$/ },
    async handler(css, id, ctx) {
      const entry = compilerFor(id, css);
      let compiler;
      try {
        compiler = await entry.compiler;
      } catch (err) {
        // A failed compile is not kept: the next build of the same text must
        // try again, not repeat a failure that a file it loads has since fixed.
        compiled.delete(id);
        ctx.error(err);
      }
      const dependsOn = [...entry.read];
      if (ctx.candidates === undefined) {
        return {
          sources: compiler.sources,
          root: compiler.root,
          utilities: compiler.features === undefined || (compiler.features & UTILITIES) !== 0,
          dependsOn,
        };
      }
      try {
        return { code: compiler.build(ctx.candidates), dependsOn };
      } catch (err) {
        ctx.error(err);
      }
    },
  },
};

await host([tailwind]);
