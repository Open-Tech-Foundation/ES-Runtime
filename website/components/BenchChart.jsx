// A dependency-free horizontal bar chart driven by bench/run.sh JSON output
// (website/src/benchmarks.js). The winner of each row (the best value in that
// metric's better direction) is drawn in green, everyone else in neutral grey.
// Pass `metrics` as [{ key, label, unit? }] selecting rows to show.
//
// NOTE: the @opentf/web compiler rewrites every `.map()` into a reactive list
// helper, so non-render computations must use plain loops (never `.map`), and
// dynamic styles must be objects (a style string becomes Object.assign(...,str)).
import bench from "../src/benchmarks.js";
import { betterLabel, winnerOf } from "../src/metric-direction.js";

const ORDER = ["esrun", "bun", "node", "deno", "llrt"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };

// Above this run-to-run variation a cell is marked `~`. The harness already
// flags these in its terminal output and publishes results_cov for every cell;
// without surfacing it here a wobbly number renders identically to a firm one,
// which is the reader assuming a precision the run never claimed.
const NOISY_COV = 10;

function maxOf(row, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v === "number" && v > max) max = v;
  }
  return max || 1;
}

function covOf(key, rt) {
  const row = bench.results_cov ? bench.results_cov[key] : null;
  const c = row ? row[rt] : null;
  return typeof c === "number" ? c : null;
}

// Whether any cell drawn by this chart is noisy, so the footnote appears only
// where it explains something. Plain loops: see the compiler NOTE above.
function hasNoisyCell(metrics, runtimes) {
  for (const m of metrics) {
    for (const rt of runtimes) {
      const c = covOf(m.key, rt);
      if (c !== null && c > NOISY_COV) return true;
    }
  }
  return false;
}

export default function BenchChart({ metrics }) {
  const runtimes = ORDER.filter((rt) => bench.runtimes[rt]);
  const showNoiseNote = hasNoisyCell(metrics, runtimes);

  return (
    <div className="space-y-5">
      {metrics.map((m) => {
        const row = bench.results_ms[m.key] || {};
        const rssRow = bench.results_rss ? bench.results_rss[m.key] : null;
        const max = maxOf(row, runtimes);
        const winner = winnerOf(row, runtimes, m.key);
        const unit = m.unit || "ms";

        return (
          <div>
            <div className="mb-1.5 flex items-baseline justify-between">
              <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
                {m.label}
              </span>
              <span className="text-[10px] text-zinc-400">{betterLabel(m.key)}</span>
            </div>
            <div className="space-y-1.5">
              {runtimes.map((rt) => {
                const v = row[rt];
                const pct =
                  typeof v === "number" ? Math.max((v / max) * 100, 2) : 0;
                const isWin = rt === winner;
                const mem = rssRow && rssRow[rt] ? ` / ${rssRow[rt]}MB` : "";
                const cov = covOf(m.key, rt);
                const noisy = cov !== null && cov > NOISY_COV;
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
                          ? "w-24 shrink-0 whitespace-nowrap text-right text-[11px] font-semibold tabular-nums text-emerald-700"
                          : "w-24 shrink-0 whitespace-nowrap text-right text-[11px] tabular-nums text-zinc-500"
                      }
                    >
                      {typeof v === "number" ? v + unit : "—"}
                      {mem}
                      {noisy ? (
                        <span
                          className="ml-0.5 font-normal text-amber-600"
                          title={`varied ${cov}% run to run — read as approximate`}
                        >
                          ~
                        </span>
                      ) : null}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        );
      })}
      {showNoiseNote ? (
        <p className="text-[10px] text-zinc-400">
          <span className="text-amber-600">~</span> varied more than {NOISY_COV}% run to
          run; read as approximate.
        </p>
      ) : null}
    </div>
  );
}
