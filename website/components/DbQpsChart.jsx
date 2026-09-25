// Database QPS chart: queries/sec and peak memory for the 100-rows-x-100
// in-flight scan per runtime, for one database — `db` is "pg" or "mysql".
// Same visual language as RpsChart (rows are runtimes, bar columns are
// metrics, the winner of each column is drawn bold); data comes from
// bench/db/<db>/qps-run.sh via bench.results_<db>_qps.
//
// NOTE: same compiler constraint as RpsChart — non-render computations use
// plain loops, dynamic styles are objects.
import bench from "../src/benchmarks.js";
import { LABELS, ORDER } from "../src/runtimes.js";

const BRAND = {
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
  esrun: {
    bar: "bg-orange-500 dark:bg-orange-400",
    text: "text-orange-700 dark:text-orange-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
  node: {
    bar: "bg-teal-600 dark:bg-teal-400",
    text: "text-teal-700 dark:text-teal-400 font-bold",
    dimText: "text-zinc-600 dark:text-zinc-300 font-medium",
  },
};

const TITLES = { pg: "Postgres", mysql: "MySQL" };

function qpsOf(db) {
  return bench[`results_${db}_qps`]?.[`${db}_qps`] ?? null;
}

function rssOf(db) {
  return bench[`results_${db}_qps_rss`]?.[`${db}_qps`] ?? null;
}

function getMax(f, runtimes) {
  let max = 0;
  for (const rt of runtimes) {
    const v = f(rt);
    if (typeof v === "number" && v > max) max = v;
  }
  return max;
}

// Higher wins for qps, lower wins for rss: the comparator decides.
function getWinner(f, runtimes, higher) {
  let best = higher ? 0 : Infinity;
  let winner = null;
  for (const rt of runtimes) {
    const v = f(rt);
    if (typeof v !== "number") continue;
    if ((higher && v > best) || (!higher && v < best)) {
      best = v;
      winner = rt;
    }
  }
  return winner;
}

function getPct(f, runtimes, rt) {
  const v = f(rt);
  const max = getMax(f, runtimes);
  if (typeof v !== "number" || !max) return 0;
  return Math.max((v / max) * 100, 2);
}

function fmtQps(v) {
  return typeof v === "number" ? v.toLocaleString("en-US") : "n/a";
}

function fmtMb(v) {
  return typeof v === "number" ? v + " MB" : "n/a";
}

export default function DbQpsChart({ db = "pg", large = false }) {
  const qps = qpsOf(db);
  const rss = rssOf(db);
  if (!qps) return null;
  const getQps = (rt) => qps[rt] ?? null;
  const getRss = (rt) => rss?.[rt] ?? null;
  // Ranked fastest first; a runtime without a number sinks to the bottom.
  const runtimes = ORDER.filter((rt) => qps[rt] !== undefined)
    .slice()
    .sort((a, b) => {
      const va = getQps(a);
      const vb = getQps(b);
      if (typeof va !== "number") return 1;
      if (typeof vb !== "number") return -1;
      return vb - va;
    });
  if (runtimes.length === 0) return null;

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

  const qpsWin = getWinner(getQps, runtimes, true);
  const rssWin = getWinner(getRss, runtimes, false);

  return (
    <div>
      <div className="mb-3 flex items-center justify-between">
        <span className={titleCls}>{TITLES[db] ?? db} · 100 rows × 100 in flight</span>
      </div>

      <div className={"mb-2 grid grid-cols-12 gap-2 " + headCls}>
        <div className="col-span-3">Runtime</div>
        <div className="col-span-5 text-left">Queries/sec (higher ↑)</div>
        <div className="col-span-4 text-left">Peak memory (lower ↓)</div>
      </div>

      <div className={large ? "space-y-3" : "space-y-2"}>
        {runtimes.map((rt) => {
          const brand = BRAND[rt] || {
            bar: "bg-zinc-400 dark:bg-zinc-500",
            text: "text-zinc-900 dark:text-zinc-100 font-semibold",
            dimText: "text-zinc-500 tabular-nums",
          };
          return (
            <div className="grid grid-cols-12 items-center gap-2">
              <div className={"col-span-3 " + nameCls}>{LABELS[rt] || rt}</div>
              <div className="col-span-5 flex items-center gap-1.5 pr-1">
                <div className={barCls}>
                  <div
                    className={"h-full rounded-full " + brand.bar}
                    style={{ width: getPct(getQps, runtimes, rt) + "%" }}
                  />
                </div>
                <span className={(large ? "w-14 " : "w-11 ") + valCls + (rt === qpsWin ? brand.text : brand.dimText)}>
                  {fmtQps(getQps(rt))}
                </span>
              </div>
              <div className="col-span-4 flex items-center gap-1.5">
                <div className={barCls}>
                  <div
                    className={"h-full rounded-full opacity-80 " + brand.bar}
                    style={{ width: getPct(getRss, runtimes, rt) + "%" }}
                  />
                </div>
                <span className={(large ? "w-16 " : "w-12 ") + valCls + (rt === rssWin ? brand.text : brand.dimText)}>
                  {fmtMb(getRss(rt))}
                </span>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
