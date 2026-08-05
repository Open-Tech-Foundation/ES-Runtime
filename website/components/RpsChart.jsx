// Higher-is-better companion to BenchChart for the HTTP requests/sec result,
// read from the same generated module as every other chart — bench/rps.sh
// writes `results_rps` into it via bench/gen-bench-data.sh.
//
// Interactive tabbed switcher with brand colors allowing users to view Throughput,
// Sustained performance, Memory consumption, and Efficiency (req/s per MB).
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

function fmtNum(v) {
  return typeof v === "number" ? Math.round(v).toLocaleString() : "n/a";
}

function getRawVal(server, rt, tabId) {
  const httpRps = bench.results_rps?.[server];
  const sustainedRps = bench.results_rps?.[server + "_sustained"];
  const serverRss = bench.results_rps_rss?.[server];

  if (tabId === "burst") return httpRps?.[rt] ?? null;
  if (tabId === "sustained") return sustainedRps?.[rt] ?? null;
  if (tabId === "memory") return serverRss?.[rt] ?? null;
  if (tabId === "efficiency") {
    const rps = httpRps?.[rt];
    const rss = serverRss?.[rt];
    return rps && rss ? rps / rss : null;
  }
  return null;
}

function getMaxVal(server, runtimes, tabId) {
  let max = 0;
  for (const rt of runtimes) {
    const v = getRawVal(server, rt, tabId);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

function getWinner(server, runtimes, tabId) {
  let best = tabId === "memory" ? Infinity : 0;
  let winner = null;
  for (const rt of runtimes) {
    const v = getRawVal(server, rt, tabId);
    if (typeof v === "number") {
      if (tabId === "memory") {
        if (v < best) {
          best = v;
          winner = rt;
        }
      } else {
        if (v > best) {
          best = v;
          winner = rt;
        }
      }
    }
  }
  return winner;
}

function getPct(server, runtimes, rt, tabId) {
  const v = getRawVal(server, rt, tabId);
  const max = getMaxVal(server, runtimes, tabId);
  if (typeof v !== "number" || !max) return 0;
  return Math.max((v / max) * 100, 2);
}

function getFormattedVal(server, rt, tabId) {
  const v = getRawVal(server, rt, tabId);
  if (typeof v !== "number") return "n/a";
  if (tabId === "burst" || tabId === "sustained") return fmtRps(v);
  if (tabId === "memory") return v + " MB";
  if (tabId === "efficiency") return fmtNum(v) + " /MB";
  return "n/a";
}

export default function RpsChart({ server = "hono", title = "HTTP requests/sec · Hono hello-world" }) {
  let activeTab = $state("burst");

  const httpRps = bench.results_rps ? bench.results_rps[server] : null;
  if (!httpRps) return null;

  const sustainedRps = bench.results_rps ? bench.results_rps[server + "_sustained"] : null;
  const serverRss = bench.results_rps_rss?.[server] || {};

  const runtimes = ORDER.filter((rt) => typeof httpRps[rt] === "number");
  if (runtimes.length === 0) return null;

  const tabs = [
    { id: "burst", label: "Throughput", direction: "higher" },
  ];
  if (sustainedRps && runtimes.some((rt) => typeof sustainedRps[rt] === "number")) {
    tabs.push({ id: "sustained", label: "60s Sustained", direction: "higher" });
  }
  if (runtimes.some((rt) => typeof serverRss[rt] === "number")) {
    tabs.push({ id: "memory", label: "Memory", direction: "lower" });
    tabs.push({ id: "efficiency", label: "Efficiency", direction: "higher" });
  }

  return (
    <div>
      <div className="mb-2 flex items-baseline justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
          {title}
        </span>
        <span className="text-[10px] text-zinc-400">
          {activeTab === "memory" ? "lower is better" : "higher is better"}
        </span>
      </div>

      {tabs.length > 1 ? (
        <div className="mb-3.5 flex items-center gap-1 overflow-x-auto rounded-lg bg-zinc-100 p-1 dark:bg-zinc-800/70">
          {tabs.map((tab) => (
            <button
              type="button"
              onclick={() => (activeTab = tab.id)}
              className={
                activeTab === tab.id
                  ? "rounded-md bg-white px-2.5 py-1 text-[11px] font-semibold text-zinc-900 shadow-sm dark:bg-zinc-700 dark:text-zinc-100 transition-all"
                  : "rounded-md px-2.5 py-1 text-[11px] font-medium text-zinc-500 hover:text-zinc-900 dark:text-zinc-400 dark:hover:text-zinc-200 transition-colors"
              }
            >
              {tab.label}
            </button>
          ))}
        </div>
      ) : null}

      <div className="space-y-1.5">
        {runtimes.map((rt) => {
          const brand = BRAND_COLORS[rt] || {
            bar: "bg-zinc-400 dark:bg-zinc-500",
            text: "text-zinc-900 dark:text-zinc-100 font-semibold",
            dimText: "text-zinc-500 tabular-nums",
          };

          return (
            <div className="flex items-center gap-2.5">
              <span className="w-14 shrink-0 text-right text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
                {LABELS[rt]}
              </span>
              <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                <div
                  className={"h-full rounded-full " + brand.bar}
                  style={{ width: getPct(server, runtimes, rt, activeTab) + "%" }}
                />
              </div>
              <span
                className={
                  "w-24 shrink-0 whitespace-nowrap text-right text-[11px] tabular-nums " +
                  (rt === getWinner(server, runtimes, activeTab) ? brand.text : brand.dimText)
                }
              >
                {getFormattedVal(server, rt, activeTab)}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}



