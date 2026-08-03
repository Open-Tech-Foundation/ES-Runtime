// Generates the site's internals pages from the canonical Markdown in
// `docs/internals/`. The repo file is the source; the site page is a build
// artifact and carries a banner saying so.
//
// This exists because the same fact living in two hand-edited files is how
// documentation starts lying: this repo has already had the site's version
// string drift from Cargo.toml twice, and benchmark tables outlive the run that
// produced them. One source, one edit.
//
//   node website/scripts/gen-internals.mjs           # write the pages
//   node website/scripts/gen-internals.mjs --check    # fail if they are stale
//
// Markdown is a subset of MDX, so generation is a copy plus the front matter
// the site needs. That is deliberate: the canonical files stay readable on
// GitHub and in an editor, which they would not if they carried JSX.

import { readdirSync, readFileSync, mkdirSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const SOURCE = join(root, "docs/internals");
const OUT = join(root, "website/app/docs/internals");

const BANNER = `{/* Generated from docs/internals/%s by website/scripts/gen-internals.mjs.
    Edit that file and re-run the script; changes made here are overwritten. */}`;

/** Splits `---\n…\n---\n` front matter from the body. */
function splitFrontMatter(text, file) {
  const match = /^---\n([\s\S]*?)\n---\n/.exec(text);
  if (!match) {
    throw new Error(`${file}: needs front matter with a title and description`);
  }
  return { frontMatter: match[1], body: text.slice(match[0].length) };
}

/**
 * Markdown is a subset of MDX with one real exception: MDX has no HTML
 * comments, so `<!-- … -->` is a syntax error rather than something invisible.
 * The canonical files use them for machine-written regions (the probe table),
 * where an HTML comment is the right thing on GitHub — so translating them is
 * the generator's job, not a reason to put JSX in the source.
 */
function toMdxComments(body) {
  return body.replace(/<!--([\s\S]*?)-->/g, (_, inner) => `{/*${inner}*/}`);
}

function render(name, text) {
  const { frontMatter, body } = splitFrontMatter(text, name);
  return `---\n${frontMatter}\n---\n\n${BANNER.replace("%s", name)}\n${toMdxComments(body)}`;
}

const check = process.argv.includes("--check");
const stale = [];
let written = 0;

for (const name of readdirSync(SOURCE).filter((f) => f.endsWith(".md")).sort()) {
  const slug = name.replace(/\.md$/, "");
  const target = join(OUT, slug, "page.mdx");
  const rendered = render(name, readFileSync(join(SOURCE, name), "utf8"));
  const current = existsSync(target) ? readFileSync(target, "utf8") : null;

  if (current === rendered) continue;
  if (check) {
    stale.push(`docs/internals/${name} → app/docs/internals/${slug}/page.mdx`);
    continue;
  }
  mkdirSync(dirname(target), { recursive: true });
  writeFileSync(target, rendered);
  written += 1;
  console.log(`generated app/docs/internals/${slug}/page.mdx`);
}

if (check && stale.length) {
  console.error("Generated internals pages are out of date:");
  for (const s of stale) console.error(`  ${s}`);
  console.error("Run: node website/scripts/gen-internals.mjs");
  process.exit(1);
}
if (check) console.log("internals pages are up to date");
else if (!written) console.log("internals pages already up to date");
