// A dependency-free horizontal bar chart driven by bench/run.sh JSON output
// (site/src/benchmarks.json). Lower is better; esrun is drawn in the brand
// color. Pass `metrics` as [{ key, label, unit? }] selecting rows to show.
import bench from "../src/benchmarks.json";

// Display order: esrun first, then the runtimes we compare against.
const ORDER = ["esrun", "bun", "node", "deno"];

const LABELS = {
  esrun: "esrun",
  bun: "Bun",
  node: "Node",
  deno: "Deno",
};

function barClass(rt) {
  return rt === "esrun" ? "bg-brand-500" : "bg-zinc-300";
}
function textClass(rt) {
  return rt === "esrun" ? "font-semibold text-brand-700" : "text-zinc-500";
}

export default function BenchChart({ metrics }) {
  const runtimes = ORDER.filter((rt) => bench.runtimes[rt]);

  return (
    <div className="space-y-5">
      {metrics.map((m) => {
        const row = bench.results_ms[m.key] || {};
        const vals = runtimes
          .map((rt) => row[rt])
          .filter((v) => typeof v === "number");
        const max = vals.length ? Math.max(...vals) : 1;
        const unit = m.unit || "ms";

        return (
          <div>
            <div className="mb-1.5 flex items-baseline justify-between">
              <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
                {m.label}
              </span>
              <span className="text-[10px] text-zinc-400">lower is better</span>
            </div>
            <div className="space-y-1">
              {runtimes.map((rt) => {
                const v = row[rt];
                const pct =
                  typeof v === "number" ? Math.max((v / max) * 100, 2) : 0;
                return (
                  <div className="flex items-center gap-2">
                    <span className="w-12 shrink-0 text-right text-[11px] font-medium text-zinc-600">
                      {LABELS[rt]}
                    </span>
                    <div className="h-3.5 flex-1 overflow-hidden rounded-full bg-zinc-100">
                      <div
                        className={"h-full rounded-full " + barClass(rt)}
                        style={"width:" + pct + "%"}
                      />
                    </div>
                    <span
                      className={
                        "w-14 shrink-0 text-right text-[11px] tabular-nums " +
                        textClass(rt)
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
