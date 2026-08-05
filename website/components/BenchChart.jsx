// A dependency-free horizontal bar chart driven by bench/run.sh JSON output
// (website/src/benchmarks.js). Displays side-by-side horizontal split for
// metric performance and memory footprint with brand colors per runtime.
//
// NOTE: the @opentf/web compiler rewrites every `.map()` into a reactive list
// helper, so non-render computations must use plain loops (never `.map`), and
// dynamic styles must be objects (a style string becomes Object.assign(...,str)).
import bench from "../src/benchmarks.js";
import { resolveRows } from "../src/bench-rows.js";
import { betterLabel, winnerOf } from "../src/metric-direction.js";
import { LABELS, ORDER } from "../src/runtimes.js";

const BRAND_COLORS = {
  esrun: {
    bar: "bg-orange-500 dark:bg-orange-400",
    text: "text-orange-700 dark:text-orange-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  bun: {
    bar: "bg-rose-500 dark:bg-rose-400",
    text: "text-rose-700 dark:text-rose-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  deno: {
    bar: "bg-zinc-900 dark:bg-zinc-100",
    text: "text-zinc-900 dark:text-zinc-100 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  node: {
    bar: "bg-teal-600 dark:bg-teal-400",
    text: "text-teal-700 dark:text-teal-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  llrt: {
    bar: "bg-purple-500 dark:bg-purple-400",
    text: "text-purple-700 dark:text-purple-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
};

const NOISY_COV = 10;

function maxOf(row, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = row[rt];
    if (typeof v === "number" && v > max) max = v;
  }
  return max || 1;
}

function maxRssOf(rssRow, runtimes) {
  let max = 0;
  if (!rssRow) return 0;
  for (const rt of runtimes) {
    const v = rssRow[rt];
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

function minRssOf(rssRow, runtimes) {
  let min = Infinity;
  let winner = null;
  if (!rssRow) return null;
  for (const rt of runtimes) {
    const v = rssRow[rt];
    if (typeof v === "number" && v < min) {
      min = v;
      winner = rt;
    }
  }
  return winner;
}

function covOf(key, rt) {
  const row = bench.results_cov ? bench.results_cov[key] : null;
  const c = row ? row[rt] : null;
  return typeof c === "number" ? c : null;
}

function hasNoisyCell(metrics, runtimes) {
  for (const m of metrics) {
    for (const rt of runtimes) {
      const c = covOf(m.key, rt);
      if (c !== null && c > NOISY_COV) return true;
    }
  }
  return false;
}

export default function BenchChart({ group, rows }) {
  const metrics = resolveRows({ group, rows });
  const runtimes = ORDER.filter((rt) => bench.runtimes[rt]);
  const showNoiseNote = hasNoisyCell(metrics, runtimes);

  return (
    <div className="space-y-6">
      {metrics.map((m) => {
        const row = bench.results_ms[m.key] || {};
        const rssRow = bench.results_rss ? bench.results_rss[m.key] : null;
        const max = maxOf(row, runtimes);
        const maxRss = maxRssOf(rssRow, runtimes);
        const winner = winnerOf(row, runtimes, m.key);
        const rssWinner = minRssOf(rssRow, runtimes);
        const unit = m.unit || "ms";
        const hasRss = maxRss > 0;

        return (
          <div>
            {/* Header */}
            <div className="mb-2 flex items-baseline justify-between">
              <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
                {m.label}
              </span>
            </div>

            {/* Column Headers */}
            <div className="mb-2 grid grid-cols-12 gap-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
              <div className="col-span-3">Runtime</div>
              <div className={hasRss ? "col-span-5 text-left" : "col-span-9 text-left"}>
                Performance ({betterLabel(m.key)})
              </div>
              {hasRss ? <div className="col-span-4 text-left">Memory (lower ↓)</div> : null}
            </div>

            {/* Rows */}
            <div className="space-y-2">
              {runtimes.map((rt) => {
                const v = row[rt];
                const rssVal = rssRow ? rssRow[rt] : null;
                const pct = typeof v === "number" ? Math.max((v / max) * 100, 2) : 0;
                const rssPct = typeof rssVal === "number" && maxRss ? Math.max((rssVal / maxRss) * 100, 2) : 0;

                const isWin = rt === winner;
                const isRssWin = rt === rssWinner;

                const brand = BRAND_COLORS[rt] || {
                  bar: "bg-zinc-400 dark:bg-zinc-500",
                  text: "text-zinc-900 dark:text-zinc-100 font-semibold",
                  dimText: "text-zinc-500 tabular-nums",
                };

                const cov = covOf(m.key, rt);
                const noisy = cov !== null && cov > NOISY_COV;

                return (
                  <div className="grid grid-cols-12 items-center gap-2">
                    {/* Runtime Label */}
                    <div className="col-span-3 truncate text-[11px] font-medium text-zinc-700 dark:text-zinc-300">
                      {LABELS[rt]}
                    </div>

                    {/* Performance Metric Bar & Value */}
                    <div className={(hasRss ? "col-span-5" : "col-span-9") + " flex items-center gap-1.5 pr-1"}>
                      <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                        <div
                          className={"h-full rounded-full " + brand.bar}
                          style={{ width: pct + "%" }}
                        />
                      </div>
                      <span className={"w-16 shrink-0 whitespace-nowrap text-right text-[11px] tabular-nums " + (isWin ? brand.text : brand.dimText)}>
                        {typeof v === "number" ? v + unit : "—"}
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

                    {/* Memory RSS Bar & Value */}
                    {hasRss ? (
                      <div className="col-span-4 flex items-center gap-1.5">
                        <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                          <div
                            className={"h-full rounded-full opacity-80 " + brand.bar}
                            style={{ width: rssPct + "%" }}
                          />
                        </div>
                        <span className={"w-12 shrink-0 whitespace-nowrap text-right text-[11px] tabular-nums " + (isRssWin ? brand.text : brand.dimText)}>
                          {typeof rssVal === "number" ? rssVal + "MB" : "—"}
                        </span>
                      </div>
                    ) : null}
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

