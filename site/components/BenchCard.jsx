// One benchmark as a self-contained card: the metric label + a horizontal bar
// per runtime, the best value drawn green — same convention as BenchChart.
// Used by BenchRoller for the home-page marquee. Pass `metric` as { key, label,
// unit? }; data comes from src/benchmarks.js.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so dynamic styles must be objects (a style string becomes Object.assign).
import bench from "../src/benchmarks.js";
import { betterLabel, winnerOf } from "../src/metric-direction.js";

const ORDER = ["esrun", "bun", "node", "deno", "llrt"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };

function maxOf(row, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v === "number" && v > max) max = v;
  }
  return max || 1;
}

export default function BenchCard({ metric }) {
  const runtimes = ORDER.filter((rt) => bench.runtimes[rt]);
  const row = bench.results_ms[metric.key] || {};
  const max = maxOf(row, runtimes);
  const winner = winnerOf(row, runtimes, metric.key);
  const unit = metric.unit || "ms";

  return (
    <div className="rounded-xl border border-zinc-200 bg-white p-4 shadow-sm">
      <div className="mb-2 flex items-baseline justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-700">
          {metric.label}
        </span>
        <span className="text-[10px] text-zinc-400">{betterLabel(metric.key)}</span>
      </div>
      <div className="space-y-1.5">
        {runtimes.map((rt) => {
          const v = row[rt];
          const pct = typeof v === "number" ? Math.max((v / max) * 100, 2) : 0;
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
                    ? "w-16 shrink-0 text-right text-[11px] font-semibold tabular-nums text-emerald-700"
                    : "w-16 shrink-0 text-right text-[11px] tabular-nums text-zinc-500"
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
}
