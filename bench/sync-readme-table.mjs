// Rewrites the "Representative results" table in bench/README.md from the
// generated data module, so the README's numbers come from a real run like
// everything else does.
//
// The table used to be typed in by hand and had rotted badly — it still showed
// base64 at 71.5ms, from before that workload had a Rust implementation at all,
// against 22.5 today, and rows (`fsstat_large`) that no longer exist. A document
// that quotes measurements it did not take is the exact failure the generated
// data module exists to prevent; this closes the same loop for the README.
//
// Usage: node bench/sync-readme-table.mjs   (after bench/gen-bench-data.sh)
import { readFileSync, writeFileSync } from "node:fs";

const here = new URL(".", import.meta.url).pathname;
const readmePath = `${here}README.md`;
const dataPath = `${here}../website/src/benchmarks.js`;

const data = JSON.parse(
  readFileSync(dataPath, "utf8")
    .replace(/^\/\/.*\n/gm, "")
    .replace(/^export default /, ""),
);

const ORDER = ["node", "bun", "deno", "llrt", "esrun"];
const runtimes = ORDER.filter((rt) => data.runtimes?.[rt]);

// Every row the run publishes, in its own order — so a row added to the suite
// appears here without anyone remembering to add it.
const rows = Object.entries(data.rows || {})
  .filter(([, meta]) => meta.display !== "hidden")
  .map(([key]) => key);

const W = 9;
const cell = (s) => String(s).padStart(W);
const lines = [];
lines.push(`${"workload".padEnd(14)}|${runtimes.map((rt) => cell(rt)).join(" |")}`);
lines.push(`${"-".repeat(14)}+${runtimes.map(() => "-".repeat(W)).join("-+")}-`);
for (const key of rows) {
  const row = data.results_ms[key];
  if (!row) continue;
  const values = runtimes.map((rt) =>
    cell(typeof row[rt] === "number" ? row[rt].toFixed(1) : "n/a"),
  );
  lines.push(`${key.padEnd(14)}|${values.join(" |")}`);
}

const env = data.environment || {};
// Each runtime reports its version in its own shape, and some repeat their own
// name ("deno 2.8.3 (stable, …)"); take the first version-looking token so the
// line reads as one list rather than five different formats.
const versions = runtimes
  .map((rt) => {
    const raw = String(data.runtimes[rt] || "");
    const v = raw.match(/v?\d+\.\d+[\w.-]*/);
    return `${rt} ${v ? v[0] : raw}`;
  })
  .join(", ");
const block = [
  "<!-- generated: bench/sync-readme-table.mjs — do not edit by hand -->",
  "",
  "Times in **milliseconds, lower is better** (`rss`/`rss_loaded` in MB), from the",
  "same run that feeds the site. One machine; re-run locally for your own numbers.",
  "",
  "```",
  ...lines,
  "```",
  "",
  `${env.cpu || "unknown CPU"}, ${env.cores || "?"} cores, ${env.os || ""} ${env.arch || ""}, ${env.filesystem || "?"}.`,
  "",
  `Measured: ${versions}. \`n/a\` = an API the runtime lacks, or a row it timed out on.`,
  "",
  "<!-- /generated -->",
].join("\n");

const readme = readFileSync(readmePath, "utf8");
const start = readme.indexOf("## Representative results");
const end = readme.indexOf("## Interpretation");
if (start < 0 || end < 0 || end < start) {
  console.error(
    "could not find the 'Representative results' section (bounded by '## Interpretation') in bench/README.md",
  );
  process.exit(1);
}
const updated = `${readme.slice(0, start)}## Representative results\n\n${block}\n\n${readme.slice(end)}`;
writeFileSync(readmePath, updated);
console.error(`bench/README.md: rewrote ${rows.length} rows from ${dataPath}`);
