// Per-metric "which way is better", read from the benchmark data rather than
// kept here. bench/run.sh publishes `rows[key].better` for every row it defines,
// so a throughput row added to the harness is ranked and captioned correctly by
// the standings table, the chart winner highlight and the per-row label without
// a component changing. A key outside the catalogue falls back to
// lower-is-better, which is what every time and memory figure is.
import bench from "./benchmarks.js";

export function isHigherBetter(key) {
  return bench.rows?.[key]?.better === "higher";
}

// The per-row caption shown next to a metric.
export function betterLabel(key) {
  return isHigherBetter(key) ? "higher is better" : "lower is better";
}

// The winning runtime for a row: best value in this metric's better direction.
// Returns its key, or null if the row has no numeric values.
export function winnerOf(row, runtimes, key) {
  const higher = isHigherBetter(key);
  let best = null;
  let bestV = higher ? -Infinity : Infinity;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v !== "number") continue;
    if (higher ? v > bestV : v < bestV) {
      bestV = v;
      best = rt;
    }
  }
  return best;
}
