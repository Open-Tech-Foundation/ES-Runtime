// Production-build chart: wall time, output size and peak RSS for vite
// build, oj build, esdev build and bun build on the generated 10k-component
// React fixture. Same visual language as DevServerChart (rows are tools, bar
// columns are metrics, the winner of each column is drawn bold); data comes
// from bench/dev-server/build.mjs via bench.build_time.
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
  bun: {
    label: "Bun",
    bar: "bg-rose-500 dark:bg-rose-400",
    text: "text-rose-700 dark:text-rose-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
};

const ORDER = ["vite", "oj", "esdev", "bun"];

function fmtMs(v) {
  if (typeof v !== "number") return "n/a";
  return v >= 1000 ? (v / 1000).toFixed(1) + "s" : Math.round(v) + "ms";
}

function fmtKb(v) {
  if (typeof v !== "number") return "n/a";
  return v >= 1024 ? (v / 1024).toFixed(1) + " MB" : v + " KB";
}

function fmtMb(v) {
  if (typeof v !== "number") return "n/a";
  return v >= 1024 ? (v / 1024).toFixed(1) + " GB" : v + " MB";
}

function getVal(tool, key) {
  return bench.build_time?.[tool]?.[key] ?? null;
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
  { key: "build_ms", title: "Build time (lower ↓)", fmt: fmtMs },
  { key: "out_kb", title: "Output size (lower ↓)", fmt: fmtKb },
  { key: "peak_mb", title: "Peak memory (lower ↓)", fmt: fmtMb },
];

export default function BuildChart({ large = false }) {
  if (!bench.build_time) return null;
  // Fastest build first; a tool without a number sinks to the bottom.
  const tools = ORDER.filter((t) => bench.build_time[t])
    .slice()
    .sort((a, b) => {
      const va = getVal(a, "build_ms");
      const vb = getVal(b, "build_ms");
      if (typeof va !== "number") return 1;
      if (typeof vb !== "number") return -1;
      return va - vb;
    });

  // Roomier type and taller bars for the full-viewport Benchmarks section.
  const titleCls = large
    ? "text-sm font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400"
    : "text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-400";
  const headCls = large
    ? "text-xs font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500"
    : "text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500";
  const nameCls = large
    ? "truncate text-sm font-medium text-zinc-700 dark:text-zinc-300"
    : "truncate text-[11px] font-medium text-zinc-700 dark:text-zinc-300";
  const barCls = large ? "h-4 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800" : "h-3 flex-1 overflow-hidden rounded-full bg-zinc-100 dark:bg-zinc-800";
  const valCls = large ? "shrink-0 text-right text-sm tabular-nums " : "shrink-0 text-right text-[11px] tabular-nums ";

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <span className={titleCls}>
          Production build · 10,000 components
        </span>
      </div>

      <div className={"mb-2 grid grid-cols-12 gap-2 " + headCls}>
        <div className="col-span-3">Tool</div>
        <div className="col-span-3 text-left">Build time (lower ↓)</div>
        <div className="col-span-3 text-left">Output size (lower ↓)</div>
        <div className="col-span-3 text-left">Peak memory (lower ↓)</div>
      </div>

      <div className={large ? "space-y-3" : "space-y-2"}>
        {tools.map((tool) => {
          const meta = TOOL_META[tool] || {
            label: tool,
            bar: "bg-zinc-400 dark:bg-zinc-500",
            text: "text-zinc-900 dark:text-zinc-100 font-semibold",
            dimText: "text-zinc-500 tabular-nums",
          };
          return (
            <div className="grid grid-cols-12 items-center gap-2">
              <div className={"col-span-3 " + nameCls}>
                {meta.label}
              </div>
              {COLUMNS.map((col) => {
                const isWin = tool === getWinner(tools, col.key);
                return (
                  <div className="col-span-3 flex items-center gap-1.5 pr-1">
                    <div className={barCls}>
                      <div
                        className={"h-full rounded-full " + meta.bar}
                        style={{ width: getPct(tools, col.key, tool) + "%" }}
                      />
                    </div>
                    <span
                      className={
                        (large ? "w-16 " : "w-14 ") + valCls +
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
