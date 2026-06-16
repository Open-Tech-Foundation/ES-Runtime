// A quick-glance standings table computed from src/benchmarks.js: for each
// comparable metric the runtimes are ranked (by that metric's better direction),
// and we tally how many 1st/2nd/3rd/… places each took. A runtime is ranked only
// on metrics it can actually run, so the "Tests" totals differ (e.g. LLRT has no
// HTTP server or fs here) — that's shown, not hidden. Unbiased by construction:
// it's just the measured numbers, sorted.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so all of the tallying below uses plain loops; `.map` appears only in render.
import bench from "../src/benchmarks.js";
import { isHigherBetter } from "../src/metric-direction.js";

const ORDER = ["esrun", "bun", "node", "deno", "llrt"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };


function medalHead(p) {
  if (p === 1) return "🥇";
  if (p === 2) return "🥈";
  if (p === 3) return "🥉";
  return p + "th";
}

export default function BenchStandings() {
  const runtimes = [];
  for (const r of ORDER) if (bench.runtimes[r]) runtimes.push(r);
  const n = runtimes.length;
  const metrics = Object.keys(bench.results_ms);

  // tally[r] = { tests, pos: [_, c1, c2, …, cn] }
  const tally = {};
  for (const r of runtimes) tally[r] = { tests: 0, pos: new Array(n + 1).fill(0) };
  for (const m of metrics) {
    const row = bench.results_ms[m];
    const ranked = [];
    for (const r of runtimes) {
      const v = row[r];
      if (typeof v === "number") ranked.push([r, v]);
    }
    const higher = isHigherBetter(m);
    ranked.sort((a, b) => (higher ? b[1] - a[1] : a[1] - b[1]));
    for (let i = 0; i < ranked.length; i++) {
      const r = ranked[i][0];
      tally[r].tests += 1;
      tally[r].pos[i + 1] += 1;
    }
  }

  const positions = [];
  for (let i = 1; i <= n; i++) positions.push(i);

  return (
    <div>
      <div className="overflow-x-auto rounded-xl border border-zinc-200">
        <table className="w-full border-collapse text-sm">
          <thead>
            <tr className="bg-zinc-50 text-zinc-500">
              <th className="px-3 py-2 text-left font-semibold">Runtime</th>
              <th className="px-3 py-2 text-right font-medium" title="Metrics this runtime could run">
                Tests
              </th>
              {positions.map((p) => (
                <th className="px-3 py-2 text-right font-medium" title={p + (p === 1 ? "st" : p === 2 ? "nd" : p === 3 ? "rd" : "th") + " places"}>
                  {medalHead(p)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {runtimes.map((r) => {
              const t = tally[r];
              return (
                <tr className="border-t border-zinc-100">
                  <td className="px-3 py-2 font-semibold text-zinc-900">
                    {LABELS[r]}
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums text-zinc-500">{t.tests}</td>
                  {positions.map((p) => {
                    const c = t.pos[p];
                    const win = p === 1 && c > 0;
                    return (
                      <td
                        className={
                          win
                            ? "px-3 py-2 text-right font-semibold tabular-nums text-emerald-700"
                            : "px-3 py-2 text-right tabular-nums text-zinc-600"
                        }
                      >
                        {c || "·"}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <p className="mt-2 text-xs text-zinc-400">
        Across {metrics.length} comparable metrics, each ranked by its own better
        direction. Each runtime is ranked only on metrics it can run, so totals
        differ — e.g. LLRT has no HTTP server or filesystem here. Place counts are
        not a ranking: more 1st-place finishes does not mean “best overall.”
      </p>

    </div>
  );
}
