// Builds src/index.ts into dist/ — one ESM bundle plus its declarations.
//
// Bundled rather than emitted file-per-module because the whole package is a
// single import in practice, and because `runtime:` specifiers must survive the
// build untouched: they are resolved by the runtime, not by a bundler, and a
// bundler that tried to follow them would fail rather than leave them alone.
import { $ } from "bun";

const EXTERNAL = ["runtime:db", "runtime:net", "runtime:process"];

const result = await Bun.build({
  entrypoints: ["./src/index.ts"],
  outdir: "./dist",
  format: "esm",
  target: "node",
  external: EXTERNAL,
});

if (!result.success) {
  for (const log of result.logs) console.error(log);
  process.exit(1);
}

await $`tsc`;
console.log(`built ${result.outputs.map((o) => o.path).join(", ")}`);
