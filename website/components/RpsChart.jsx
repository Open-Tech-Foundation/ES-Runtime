// Higher-is-better companion to BenchChart for the HTTP requests/sec result,
// read from the same generated module as every other chart — bench/rps.sh
// writes `results_rps` into it via bench/gen-bench-data.sh.
//
// This once carried an inline fallback of hand-written numbers for when the key
// was missing. It is gone on purpose: those numbers went stale (they showed
// esrun ~43% faster than it measures today) and, being a `||` fallback, they
// would have appeared on the homepage the moment a run failed to produce the
// key — silently replacing a measurement with a flattering guess. Missing data
// now renders nothing at all.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so non-render computations must use plain loops, and dynamic styles must be
// objects (a style string becomes Object.assign(..., str)).
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno" };

import bench from "../src/benchmarks.js";

const ORDER = ["deno", "bun", "esrun", "node"];

// A runtime the run could not measure reads as "n/a", never as 0.0k — a zero
// bar claims a result that was never taken.
function fmt(v) {
  return typeof v === "number" ? (v / 1000).toFixed(1) + "k" : "n/a";
}

export default function RpsChart() {
  const httpRps = bench.results_rps?.hono;
  if (!httpRps) return null;

  let max = 0;
  let winner = null;
  for (const rt of ORDER) {
    if (httpRps[rt] > max) {
      max = httpRps[rt];
      winner = rt;
    }
  }
  if (!max) return null;

  const httpRss = bench.results_rss?.http || {};

  return (
    <div>
      <div className="mb-1.5 flex items-baseline justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
          HTTP requests/sec · Hono hello-world
        </span>
        <span className="text-[10px] text-zinc-400">higher is better</span>
      </div>
      <div className="space-y-1.5">
        {ORDER.map((rt) => {
          const pct =
            typeof httpRps[rt] === "number" ? Math.max((httpRps[rt] / max) * 100, 2) : 0;
          const isWin = rt === winner;
          const mem = httpRss[rt] ? ` / ${httpRss[rt]}MB` : "";
          return (
            <div className="flex items-center gap-2.5">
              <span className="w-14 shrink-0 text-right text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                {LABELS[rt]}
              </span>
              <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                <div
                  className={
                    isWin
                      ? "h-full rounded-full bg-emerald-500"
                      : "h-full rounded-full bg-zinc-300 dark:bg-zinc-600"
                  }
                  style={{ width: pct + "%" }}
                />
              </div>
              <span
                className={
                  isWin
                    ? "w-20 shrink-0 text-right text-[11px] font-semibold tabular-nums text-emerald-700 dark:text-emerald-400"
                    : "w-20 shrink-0 text-right text-[11px] tabular-nums text-zinc-500"
                }
              >
                {fmt(httpRps[rt])}
                {mem}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
