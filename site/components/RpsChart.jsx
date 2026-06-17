// Higher-is-better companion to BenchChart for the HTTP requests/sec result
// (bench/rps.sh). Data is inline because it comes from an external load
// generator (autocannon), not the bench/run.sh JSON the other charts read.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so non-render computations must use plain loops, and dynamic styles must be
// objects (a style string becomes Object.assign(..., str)).
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno" };

import bench from "../src/benchmarks.js";

const ORDER = ["deno", "bun", "esrun", "node"];

function fmt(v) {
  return (v / 1000).toFixed(1) + "k";
}

export default function RpsChart() {
  const httpRps = bench.results_rps?.hono || { deno: 98000, bun: 81700, esrun: 77600, node: 36200 };

  let max = 0;
  let winner = null;
  for (const rt of ORDER) {
    if (httpRps[rt] > max) {
      max = httpRps[rt];
      winner = rt;
    }
  }

  const httpRss = bench.results_rss?.http || {};

  return (
    <div>
      <div className="mb-1.5 flex items-baseline justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
          HTTP requests/sec · Hono hello-world
        </span>
        <span className="text-[10px] text-zinc-400">higher is better</span>
      </div>
      <div className="space-y-1.5">
        {ORDER.map((rt) => {
          const pct = Math.max((httpRps[rt] / max) * 100, 2);
          const isWin = rt === winner;
          const mem = httpRss[rt] ? ` / ${httpRss[rt]}MB` : "";
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
                    ? "w-20 shrink-0 text-right text-[11px] font-semibold tabular-nums text-emerald-700"
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
