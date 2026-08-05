// Companion chart displaying side-by-side horizontal comparison of Throughput
// and Memory footprint for HTTP servers (Hono / static file).
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so non-render computations must use plain loops, and dynamic styles must be
// objects (a style string becomes Object.assign(..., str)).
import bench from "../src/benchmarks.js";
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

function fmtRps(v) {
  return typeof v === "number" ? (v / 1000).toFixed(1) + "k" : "n/a";
}

function getRpsVal(server, rt, mode) {
  const dataset = mode === "sustained"
    ? bench.results_rps?.[server + "_sustained"]
    : bench.results_rps?.[server];
  return dataset?.[rt] ?? null;
}

function getMaxRps(server, runtimes, mode) {
  let max = 0;
  for (const rt of runtimes) {
    const v = getRpsVal(server, rt, mode);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

function getRpsWinner(server, runtimes, mode) {
  let max = 0;
  let winner = null;
  for (const rt of runtimes) {
    const v = getRpsVal(server, rt, mode);
    if (typeof v === "number" && v > max) {
      max = v;
      winner = rt;
    }
  }
  return winner;
}

function getRpsPct(server, runtimes, rt, mode) {
  const v = getRpsVal(server, rt, mode);
  const max = getMaxRps(server, runtimes, mode);
  if (typeof v !== "number" || !max) return 0;
  return Math.max((v / max) * 100, 2);
}

function getFormattedRps(server, rt, mode) {
  const v = getRpsVal(server, rt, mode);
  return fmtRps(v);
}

function getRssVal(server, rt) {
  const serverRss = bench.results_rps_rss?.[server];
  return serverRss?.[rt] ?? null;
}

function getMaxRss(server, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = getRssVal(server, rt);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

function getRssWinner(server, runtimes) {
  let min = Infinity;
  let winner = null;
  for (const rt of runtimes) {
    const v = getRssVal(server, rt);
    if (typeof v === "number" && v < min) {
      min = v;
      winner = rt;
    }
  }
  return winner;
}

function getRssPct(server, runtimes, rt) {
  const v = getRssVal(server, rt);
  const max = getMaxRss(server, runtimes);
  if (typeof v !== "number" || !max) return 0;
  return Math.max((v / max) * 100, 2);
}

function getFormattedRss(server, rt) {
  const v = getRssVal(server, rt);
  return typeof v === "number" ? v + " MB" : "n/a";
}

export default function RpsChart({ server = "hono", title = "Hono hello-world · Speed & Memory" }) {
  let mode = $state("burst");

  const httpRps = bench.results_rps ? bench.results_rps[server] : null;
  if (!httpRps) return null;

  const sustainedRps = bench.results_rps ? bench.results_rps[server + "_sustained"] : null;
  const runtimes = ORDER.filter((rt) => typeof httpRps[rt] === "number");
  if (runtimes.length === 0) return null;

  return (
    <div>
      {/* Title & Mode Switcher */}
      <div className="mb-3 flex items-center justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
          {title}
        </span>
        {sustainedRps && runtimes.some((rt) => typeof sustainedRps[rt] === "number") ? (
          <div className="flex items-center gap-1 rounded-md bg-zinc-100 p-0.5 dark:bg-zinc-800/80">
            <button
              type="button"
              onclick={() => (mode = "burst")}
              className={
                mode === "burst"
                  ? "rounded px-2 py-0.5 text-[10px] font-semibold text-zinc-900 bg-white shadow-xs dark:bg-zinc-700 dark:text-zinc-100 transition-all"
                  : "rounded px-2 py-0.5 text-[10px] font-medium text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200 transition-colors"
              }
            >
              Burst
            </button>
            <button
              type="button"
              onclick={() => (mode = "sustained")}
              className={
                mode === "sustained"
                  ? "rounded px-2 py-0.5 text-[10px] font-semibold text-zinc-900 bg-white shadow-xs dark:bg-zinc-700 dark:text-zinc-100 transition-all"
                  : "rounded px-2 py-0.5 text-[10px] font-medium text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200 transition-colors"
              }
            >
              60s Sustained
            </button>
          </div>
        ) : null}
      </div>

      {/* Column Headers */}
      <div className="mb-2 grid grid-cols-12 gap-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
        <div className="col-span-3">Runtime</div>
        <div className="col-span-5 text-left">Throughput (higher ↑)</div>
        <div className="col-span-4 text-left">Memory (lower ↓)</div>
      </div>

      {/* Side-by-side Runtimes List */}
      <div className="space-y-2">
        {runtimes.map((rt) => {
          const brand = BRAND_COLORS[rt] || {
            bar: "bg-zinc-400 dark:bg-zinc-500",
            text: "text-zinc-900 dark:text-zinc-100 font-semibold",
            dimText: "text-zinc-500 tabular-nums",
          };

          const isRpsWin = rt === getRpsWinner(server, runtimes, mode);
          const isRssWin = rt === getRssWinner(server, runtimes);

          return (
            <div className="grid grid-cols-12 items-center gap-2">
              {/* Runtime Name */}
              <div className="col-span-3 truncate text-[11px] font-medium text-zinc-700 dark:text-zinc-300">
                {LABELS[rt]}
              </div>

              {/* Throughput Bar & Value */}
              <div className="col-span-5 flex items-center gap-1.5 pr-1">
                <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                  <div
                    className={"h-full rounded-full " + brand.bar}
                    style={{ width: getRpsPct(server, runtimes, rt, mode) + "%" }}
                  />
                </div>
                <span
                  className={
                    "w-11 shrink-0 text-right text-[11px] tabular-nums " +
                    (isRpsWin ? brand.text : brand.dimText)
                  }
                >
                  {getFormattedRps(server, rt, mode)}
                </span>
              </div>

              {/* Memory Bar & Value */}
              <div className="col-span-4 flex items-center gap-1.5">
                <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                  <div
                    className={"h-full rounded-full opacity-80 " + brand.bar}
                    style={{ width: getRssPct(server, runtimes, rt) + "%" }}
                  />
                </div>
                <span
                  className={
                    "w-12 shrink-0 text-right text-[11px] tabular-nums " +
                    (isRssWin ? brand.text : brand.dimText)
                  }
                >
                  {getFormattedRss(server, rt)}
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}




