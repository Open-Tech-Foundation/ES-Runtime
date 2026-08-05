// Row selection for the benchmark components, resolved from the harness's own
// catalogue (`rows` + `groups` in src/benchmarks.js, written by bench/run.sh).
//
// Nothing on the site names a metric's label, unit or order any more: a chart
// asks for a group and gets the rows that group holds, in the order the harness
// defines them. Adding a row to bench/run.sh is the whole of adding it here.
import bench from "./benchmarks.js";

const CATALOGUE = bench.rows || {};
const GROUPS = bench.groups || [];

function metricFor(key) {
  const meta = CATALOGUE[key];
  if (!meta) return null;
  return { key, label: meta.label, unit: meta.unit, group: meta.group };
}

// Accepts a space-separated group list, a space-separated row list, or both;
// unknown names resolve to nothing rather than throwing, so a stale reference
// renders empty instead of taking the page down. The validator in
// bench/validate-bench-data.mjs is what catches it before publication.
export function resolveRows({ group, rows } = {}) {
  const out = [];
  const seen = new Set();
  const push = (key) => {
    const m = metricFor(key);
    if (m && !seen.has(key)) {
      seen.add(key);
      out.push(m);
    }
  };
  for (const id of String(group || "").split(/\s+/).filter(Boolean)) {
    const g = GROUPS.find((x) => x.id === id);
    if (g) for (const key of g.rows) push(key);
  }
  for (const key of String(rows || "").split(/\s+/).filter(Boolean)) push(key);
  return out;
}

// The rows the harness marks for the home page's card roller — a subset, since
// a shop window is not the full results table.
export function cardRows() {
  const out = [];
  for (const g of GROUPS) {
    for (const key of g.rows) {
      if (CATALOGUE[key]?.display === "card") out.push(metricFor(key));
    }
  }
  return out;
}
