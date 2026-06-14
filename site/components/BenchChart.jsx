// A dependency-free horizontal bar chart driven by bench/run.sh JSON output
// (site/src/benchmarks.json). Lower is better; the winner of each row (the
// fastest/smallest value) is drawn in green, everyone else in neutral grey.
// Pass `metrics` as [{ key, label, unit? }] selecting rows to show.
//
// NOTE: the @opentf/web compiler rewrites every `.map()` into a reactive list
// helper, so non-render computations must use plain loops (never `.map`), and
// dynamic styles must be objects (a style string becomes Object.assign(...,str)).
import bench from "../src/benchmarks.json";

const ORDER = ["esrun", "bun", "node", "deno"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno" };

function maxOf(row, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v === "number" && v > max) max = v;
  }
  return max || 1;
}

// The winning runtime for a row: the one with the lowest value (lower is
// better). Returns its key, or null if the row has no numeric values.
function winnerOf(row, runtimes) {
  let best = null;
  let bestV = Infinity;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v === "number" && v < bestV) {
      bestV = v;
      best = rt;
    }
  }
  return best;
}

export default function BenchChart({ metrics }) {
  const runtimes = ORDER.filter((rt) => bench.runtimes[rt]);

  return (
    <div className="space-y-5">
      {metrics.map((m) => {
        const row = bench.results_ms[m.key] || {};
        const max = maxOf(row, runtimes);
        const winner = winnerOf(row, runtimes);
        const unit = m.unit || "ms";

        return (
          <div>
            <div className="mb-1.5 flex items-baseline justify-between">
              <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
                {m.label}
              </span>
              <span className="text-[10px] text-zinc-400">lower is better</span>
            </div>
            <div className="space-y-1.5">
              {runtimes.map((rt) => {
                const v = row[rt];
                const pct =
                  typeof v === "number" ? Math.max((v / max) * 100, 2) : 0;
                const isWin = rt === winner;
                return (
                  <div className="flex items-center gap-2.5">
                    <span className="w-14 shrink-0 text-right text-[11px] font-medium text-zinc-600">
                      {LABELS[rt]}
                    </span>
                    <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100">
                      <div
                        className={
                          isWin
                            ? "h-full rounded-full bg-emerald-500"
                            : "h-full rounded-full bg-zinc-300"
                        }
                        style={{ width: pct + "%" }}
                      />
                    </div>
                    <span
                      className={
                        isWin
                          ? "w-14 shrink-0 text-right text-[11px] font-semibold tabular-nums text-emerald-700"
                          : "w-14 shrink-0 text-right text-[11px] tabular-nums text-zinc-500"
                      }
                    >
                      {typeof v === "number" ? v + unit : "—"}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        );
      })}
    </div>
  );
}
