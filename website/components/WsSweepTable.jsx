// WebSocket fan-out sweep table for the Benchmarks page — RECV messages/sec
// (higher is better). Columns are the C-sweep keys; rows the runtimes that
// participated. The fastest cell in each column is marked like ImagesTable's
// winners. Used by app/docs/benchmarks/page.mdx.
import { LABELS, ORDER as WS_ORDER } from "../src/runtimes.js";
const fmt = (n) => (n == null ? "n/a" : n.toLocaleString("en-US"));
// Same winner tone as ImagesTable.
const WIN = "font-semibold text-emerald-600 dark:text-emerald-400";
const PLAIN = "text-zinc-600 dark:text-zinc-400";

export default function WsSweepTable({ sweep, header }) {
  const cols = Object.keys(sweep)
    .map(Number)
    .sort((a, b) => a - b);
  const rows = WS_ORDER.filter((rt) => cols.some((c) => sweep[c]?.[rt] != null));
  // Fastest value per column; a missing cell never wins.
  const best = {};
  for (const c of cols) {
    let top = -Infinity;
    for (const rt of rows) {
      const v = sweep[c]?.[rt];
      if (typeof v === "number" && v > top) top = v;
    }
    best[c] = top;
  }
  return (
    <div className="mt-3 overflow-hidden rounded-xl border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
          <tr>
            <th className="px-4 py-3 font-semibold">{header}</th>
            {cols.map((c) => (
              <th className="px-4 py-3 text-right font-semibold">C={c}</th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100 dark:divide-zinc-800">
          {rows.map((rt) => (
            <tr>
              <td className="px-4 py-3 font-mono text-zinc-900 dark:text-zinc-100">{LABELS[rt] || rt}</td>
              {cols.map((c) => (
                <td
                  className={
                    "px-4 py-3 text-right font-mono tabular-nums " +
                    (typeof sweep[c]?.[rt] === "number" && sweep[c][rt] === best[c] ? WIN : PLAIN)
                  }
                >
                  {fmt(sweep[c]?.[rt])}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
