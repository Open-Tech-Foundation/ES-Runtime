// Dev-server startup chart: cold start, warm start and peak memory for
// vite dev, oj dev --bundle and esdev start on the generated 10k-component
// React fixture. Same visual language as RpsChart (rows are tools, bar
// columns are metrics, the winner of each column is drawn bold) — but the
// data comes from bench/dev-server rather than the req/s pipeline, so this
// reads bench.dev_server instead of bench.results_rps.
//
// NOTE: same compiler constraint as RpsChart — non-render computations use
// plain loops, dynamic styles are objects.
import bench from "../src/benchmarks.js";

const TOOL_META = {
  vite: {
    label: "Vite",
    bar: "bg-violet-500 dark:bg-violet-400",
    text: "text-violet-700 dark:text-violet-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  oj: {
    label: "oj",
    bar: "bg-sky-500 dark:bg-sky-400",
    text: "text-sky-700 dark:text-sky-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  esdev: {
    label: "esdev",
    bar: "bg-orange-500 dark:bg-orange-400",
    text: "text-orange-700 dark:text-orange-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
};

const ORDER = ["vite", "oj", "esdev"];

function fmtMs(v) {
  if (typeof v !== "number") return "n/a";
  return v >= 1000 ? (v / 1000).toFixed(1) + "s" : Math.round(v) + "ms";
}

function fmtMb(v) {
  if (typeof v !== "number") return "n/a";
  return v >= 1024 ? (v / 1024).toFixed(1) + " GB" : v + " MB";
}

function getVal(tool, key) {
  return bench.dev_server?.[tool]?.[key] ?? null;
}

function getMax(tools, key) {
  let max = 0;
  for (const t of tools) {
    const v = getVal(t, key);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

function getWinner(tools, key) {
  let best = Infinity;
  let winner = null;
  for (const t of tools) {
    const v = getVal(t, key);
    if (typeof v === "number" && v < best) {
      best = v;
      winner = t;
    }
  }
  return winner;
}

function getPct(tools, key, tool) {
  const v = getVal(tool, key);
  const max = getMax(tools, key);
  if (typeof v !== "number" || !max) return 0;
  return Math.max((v / max) * 100, 2);
}

const COLUMNS = [
  { key: "cold_ms", title: "Cold start (lower ↓)", fmt: fmtMs },
  { key: "warm_ms", title: "Warm start (lower ↓)", fmt: fmtMs },
  { key: "peak_mb", title: "Peak memory (lower ↓)", fmt: fmtMb },
];

export default function DevServerChart() {
  if (!bench.dev_server) return null;
  const tools = ORDER.filter((t) => bench.dev_server[t]);

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <span className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
          Dev-server startup · 10,000 components
        </span>
      </div>

      <div className="mb-2 grid grid-cols-12 gap-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
        <div className="col-span-3">Tool</div>
        <div className="col-span-3 text-left">Cold start (lower ↓)</div>
        <div className="col-span-3 text-left">Warm start (lower ↓)</div>
        <div className="col-span-3 text-left">Peak memory (lower ↓)</div>
      </div>

      <div className="space-y-2">
        {tools.map((tool) => {
          const meta = TOOL_META[tool] || {
            label: tool,
            bar: "bg-zinc-400 dark:bg-zinc-500",
            text: "text-zinc-900 dark:text-zinc-100 font-semibold",
            dimText: "text-zinc-500 tabular-nums",
          };
          return (
            <div className="grid grid-cols-12 items-center gap-2">
              <div className="col-span-3 truncate text-[11px] font-medium text-zinc-700 dark:text-zinc-300">
                {meta.label}
              </div>
              {COLUMNS.map((col) => {
                const isWin = tool === getWinner(tools, col.key);
                return (
                  <div className="col-span-3 flex items-center gap-1.5 pr-1">
                    <div className="h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800">
                      <div
                        className={"h-full rounded-full " + meta.bar}
                        style={{ width: getPct(tools, col.key, tool) + "%" }}
                      />
                    </div>
                    <span
                      className={
                        "w-14 shrink-0 text-right text-[11px] tabular-nums " +
                        (isWin ? meta.text : meta.dimText)
                      }
                    >
                      {col.fmt(getVal(tool, col.key))}
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
