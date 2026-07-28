// Per-metric "which way is better" for the benchmark data. Every current metric
// is a time (ms) or memory (MB) figure, so lower is better; throughput-style
// metrics (e.g. req/s) would be higher-is-better. List those keys here and the
// whole site — standings ranking, chart winner highlight, and the per-row
// label — stays correct without touching three components.
export const HIGHER_BETTER = new Set([]);

export function isHigherBetter(key) {
  return HIGHER_BETTER.has(key);
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
