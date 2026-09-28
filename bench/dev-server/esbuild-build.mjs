// esbuild leg of the production-build benchmark, invoked by build.mjs as
// `node esbuild-build.mjs <appDir> <outDir>` (one arg each, no flags).
//
// esbuild has no HTML entry mode — `bun build ./index.html` and `vite build`
// both consume the fixture's index.html directly — so this bundles
// src/main.tsx to assets/bundle.js and writes an index.html that loads it.
// The output dir then has the same shape the other legs produce (an HTML
// shell plus hashed-or-fixed JS), and build.mjs's render gate (serve the
// outdir, wait for [data-done]) applies unchanged.
//
// Like-for-like notes: --bundle --minify, jsx automatic (what vite's react
// plugin and bun both use for this fixture), and NODE_ENV defined to
// production (bun gets it via NODE_ENV=production in the environment;
// esbuild needs the define because it only replaces what it is told to).
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";

const [appDir, outDir] = process.argv.slice(2);
if (!appDir || !outDir) {
  console.error("usage: node esbuild-build.mjs <appDir> <outDir>");
  process.exit(2);
}

const require = createRequire(path.join(appDir, "package.json"));
const esbuild = require("esbuild");

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(path.join(outDir, "assets"), { recursive: true });

await esbuild.build({
  entryPoints: [path.join(appDir, "src", "main.tsx")],
  bundle: true,
  minify: true,
  format: "esm",
  platform: "browser",
  jsx: "automatic",
  define: { "process.env.NODE_ENV": '"production"' },
  outfile: path.join(outDir, "assets", "bundle.js"),
  logLevel: "warning",
});

fs.writeFileSync(
  path.join(outDir, "index.html"),
  `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <title>bench esbuild</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="./assets/bundle.js"></script>
  </body>
</html>
`
);
