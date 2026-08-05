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

// The run publishes its own row catalogue (label, unit, group, where it is
// shown); the benchmarks page names groups and lets the data fill them in. So
// the two have to agree in both directions, and this checks both:
//
//   * every group the page charts must exist in the run, or the section renders
//     empty and nobody notices until someone looks at the page;
//   * every row the run publishes must reach the page, because a measurement
//     taken and then shown nowhere is a result quietly dropped. The home page is
//     a shop window and shows a subset; the benchmarks page shows everything.
const catalogue = data.rows || {};
const groups = data.groups || [];
if (Object.keys(catalogue).length === 0) {
  errors.push("`rows` is missing — the run published no row catalogue for the site to render");
}

const mdxPath = join(siteDir, "app/docs/benchmarks/page.mdx");
const mdx = readFileSync(mdxPath, "utf8");

const charted = new Set();
for (const [, attr, value] of mdx.matchAll(/<BenchChart[^>]*?\b(group|rows)="([^"]+)"/g)) {
  for (const name of value.trim().split(/\s+/)) {
    if (attr === "rows") {
      charted.add(name);
      continue;
    }
    const g = groups.find((x) => x.id === name);
    if (!g) {
      errors.push(`the benchmarks page charts group "${name}", which the run does not define`);
      continue;
    }
    for (const key of g.rows) charted.add(key);
  }
}

if (charted.size === 0) {
  errors.push(`no <BenchChart group="..."> found in ${mdxPath} — did the page change shape?`);
}

for (const key of charted) {
  if (!(key in catalogue)) {
    errors.push(`the benchmarks page charts "${key}", which is not a row the run defines`);
  }
}
for (const [key, meta] of Object.entries(catalogue)) {
  if (meta.display !== "hidden" && !charted.has(key)) {
    errors.push(
      `row "${key}" is measured and marked display="${meta.display}" but no chart on the ` +
        `benchmarks page reaches it — add its group to the page, or mark the row hidden`,
    );
  }
}

const wanted = [...charted];

const runtimes = Object.keys(data.runtimes || {});
if (runtimes.length === 0) errors.push("runtimes is empty — no runtime was detected");

// The numbers must come from the build being shipped, not an older one.
//
// This is not hypothetical. v0.12, v0.13, v0.14 and v0.15 all published the same
// data — identical to the digit, down to `http` at 92.2ms — because it was never
// regenerated. `runtimes.esrun` said "esrun 0.9.0" on all four, so the site
// carried numbers from seven minor versions earlier while claiming to describe
// the current release, and the only clue was a version string nothing compared.
const cargoToml = readFileSync(join(siteDir, "../Cargo.toml"), "utf8");
const workspaceVersion = cargoToml.match(/^version = "([^"]+)"/m)?.[1];
const measuredVersion = String(data.runtimes?.esrun || "").match(/\d+\.\d+\.\d+/)?.[0];
if (workspaceVersion && measuredVersion && workspaceVersion !== measuredVersion) {
  errors.push(
    `these numbers were measured on esrun ${measuredVersion}, but the workspace is ` +
      `${workspaceVersion} — rebuild (\`cargo build --release -p es-runtime-cli\`) and ` +
      `re-run, or the site will describe a build nobody is shipping`,
  );
}

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
  ["results_rps.hono", () => data.results_rps?.hono, "SECTIONS=rps"],
  [
    "results_rps.hono_sustained",
    () => data.results_rps?.hono_sustained,
    "SECTIONS=rps_sustained",
  ],
  [
    "results_rps.staticserver",
    () => data.results_rps?.staticserver,
    "SECTIONS=rps_static",
  ],
  ["websocket.server", () => data.websocket?.server, "SECTIONS=websocket"],
  ["websocket.client", () => data.websocket?.client, "SECTIONS=websocket"],
  ["results_http2", () => data.results_http2, "SECTIONS=http2"],
  ["memory_safety", () => data.memory_safety, "SECTIONS=memory_safety"],
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
// Spread is disclosed; the *floor* is what gets gated.
//
// The published number is the minimum, so judging it by coefficient of variation
// asks the wrong question: one writeback stall or scheduler hiccup sends CoV past
// 100% while leaving the minimum exactly where it was — which is the whole reason
// the harness aggregates by min. What decides whether that minimum is real is
// whether anything corroborates it. `results_floor_gap` is how far the second
// -lowest sample sits above the lowest; when two samples agree the floor is a
// measurement, and when nothing comes near it the floor is a fluke.
//
// So: high CoV is reported, a lone unsupported minimum is rejected.
const COV_WOBBLY = 10;
const FLOOR_GAP_UNPUBLISHABLE = 25;

const cov = data.results_cov || {};
for (const [key, row] of Object.entries(cov)) {
  for (const rt of runtimes) {
    const c = row?.[rt];
    if (typeof c === "number" && c > COV_WOBBLY) {
      warnings.push(`${key}/${rt}: coefficient of variation ${c}% — the number is wobbly`);
    }
  }
}

const floorGap = data.results_floor_gap || {};
for (const [key, row] of Object.entries(floorGap)) {
  for (const rt of runtimes) {
    const g = row?.[rt];
    if (typeof g !== "number") continue;
    if (g > FLOOR_GAP_UNPUBLISHABLE) {
      errors.push(
        `results_floor_gap."${key}"."${rt}": the next-lowest sample is ${g}% above ` +
          `the published minimum — nothing corroborates that floor, so it is one ` +
          `lucky run rather than a measurement`,
      );
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
