// Gate between a benchmark run and the site's data module.
//
// bench/gen-bench-data.sh writes website/src/benchmarks.js from real runs, so
// the numbers are never typed by hand. That only holds if a run that went wrong
// is *rejected* — otherwise a suite that half-failed writes a module full of
// nulls, every chart quietly renders "n/a", and the fix is a human editing the
// file back. This checks the generated JSON against what the site actually
// reads, and fails the generation instead.
//
// Usage: node bench/validate-bench-data.mjs <candidate.json> [website-dir]
import { readFileSync } from "node:fs";
import { join } from "node:path";

const [, , candidatePath, siteDir = new URL("../website", import.meta.url).pathname] =
  process.argv;

if (!candidatePath) {
  console.error("usage: validate-bench-data.mjs <candidate.json> [website-dir]");
  process.exit(2);
}

const data = JSON.parse(readFileSync(candidatePath, "utf8"));
const errors = [];
const warnings = [];

// --- what the site asks for -------------------------------------------------

// The benchmarks page declares its rows as `{ key: "...", label: ... }` literals.
// Reading them back is what keeps this honest: add a row to the page and the
// generator starts requiring the run to produce it, rather than rendering blank.
const mdxPath = join(siteDir, "app/docs/benchmarks/page.mdx");
const mdx = readFileSync(mdxPath, "utf8");
const wanted = [...mdx.matchAll(/\{\s*key:\s*"([^"]+)"/g)].map((m) => m[1]);

if (wanted.length === 0) {
  errors.push(`no { key: "..." } rows found in ${mdxPath} — did the page change shape?`);
}

const runtimes = Object.keys(data.runtimes || {});
if (runtimes.length === 0) errors.push("runtimes is empty — no runtime was detected");

// --- every charted row exists, and carries real numbers ---------------------

const rows = data.results_ms || {};
for (const key of wanted) {
  if (!(key in rows)) {
    errors.push(`results_ms is missing "${key}", which the benchmarks page charts`);
    continue;
  }
  const row = rows[key];
  const measured = runtimes.filter((rt) => typeof row[rt] === "number");
  if (measured.length === 0) {
    errors.push(`results_ms."${key}" has no measured value for any runtime`);
  }
  for (const rt of runtimes) {
    const v = row[rt];
    if (v === null || v === undefined) continue; // n/a is a legitimate outcome
    if (typeof v !== "number" || !Number.isFinite(v) || v < 0) {
      errors.push(`results_ms."${key}"."${rt}" is not a usable number: ${JSON.stringify(v)}`);
    }
  }
}

// esrun is the subject of the comparison; a run where it produced nothing is a
// broken run, not a result.
const esrunRows = Object.entries(rows).filter(([, r]) => typeof r.esrun === "number");
if (runtimes.includes("esrun") && esrunRows.length === 0) {
  errors.push("esrun has no measured value in any row");
}

// --- the sections fed by the other scripts ----------------------------------

// Each entry: the path the site reads, and how to tell "present and populated".
const sections = [
  ["results_rps.hono", () => data.results_rps?.hono, "bench/rps.sh"],
  ["websocket.server", () => data.websocket?.server, "bench/websocket-chat/run-chat.sh"],
  ["websocket.client", () => data.websocket?.client, "bench/websocket-chat/run-chat.sh"],
  ["results_http2", () => data.results_http2, "bench/http2.sh"],
];
for (const [path, get, source] of sections) {
  const v = get();
  if (!v || Object.keys(v).length === 0) {
    errors.push(`${path} is missing or empty — ${source} did not produce data`);
  }
}

// --- quality signals: reported, not fatal -----------------------------------

// The run knows which cells were noisy or timed out. Surfacing them here means
// the person publishing sees them, rather than only whoever watched stderr.
const cov = data.results_cov || {};
for (const [key, row] of Object.entries(cov)) {
  for (const rt of runtimes) {
    const c = row?.[rt];
    if (typeof c === "number" && c > 10) {
      warnings.push(`${key}/${rt}: coefficient of variation ${c}% — the number is wobbly`);
    }
  }
}

const status = data.status || {};
for (const [key, row] of Object.entries(status)) {
  for (const rt of runtimes) {
    if (row?.[rt] === "timeout") {
      warnings.push(`${key}/${rt}: timed out — published as n/a, but it is not unsupported`);
    }
  }
}

// A workload that finished too fast measured the clock. It is a warning rather
// than an error because raising the workload's N is a code change, not a re-run.
// The wall-clock rows are exempt: startup and bigscript time a process launch,
// so a small number there is the finding, not an under-sized workload.
const MIN_MS = 5;
const WALL_CLOCK_ROWS = new Set(["startup", "bigscript", "rss"]);
for (const [key, row] of Object.entries(rows)) {
  if (WALL_CLOCK_ROWS.has(key)) continue;
  for (const rt of runtimes) {
    const v = row?.[rt];
    if (typeof v === "number" && v > 0 && v < MIN_MS) {
      warnings.push(`${key}/${rt}: ${v}ms is below the ${MIN_MS}ms measurement floor — raise its N`);
    }
  }
}

// --- verdict ----------------------------------------------------------------

for (const w of warnings) console.error(`  warn: ${w}`);
if (errors.length > 0) {
  console.error(`\nbenchmark data rejected (${errors.length} problem(s)):`);
  for (const e of errors) console.error(`  error: ${e}`);
  console.error("\nwebsite/src/benchmarks.js was left unchanged. Re-run the benchmark;");
  console.error("do not edit the data module by hand to work around this.");
  process.exit(1);
}
console.error(
  `benchmark data OK: ${Object.keys(rows).length} rows x ${runtimes.length} runtimes` +
    (warnings.length ? `, ${warnings.length} warning(s)` : ""),
);
