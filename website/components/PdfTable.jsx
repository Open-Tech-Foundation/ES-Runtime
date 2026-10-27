// PDF table for the Benchmarks page: wall ms or peak RSS, lower is better.
// Data comes from bench/pdf/run.sh via gen-bench-data.sh. Used by
// app/docs/benchmarks/page.mdx.

import { LABELS, ORDER } from "../src/runtimes.js";

const WORKLOADS = {
  text: "Text of a 200-page report",
  merge: "Merge it with itself (400 pages)",
  split: "Split off its first 10 pages",
};

// esrun's is built in; the others run the npm libraries applications use.
const LIBRARY = { node: "pdf.js + pdf-lib", deno: "pdf.js + pdf-lib", bun: "pdf.js + pdf-lib", esrun: "runtime:pdf" };

const METRICS = {
  ms: (v) => `${Math.round(v).toLocaleString("en-US")} ms`,
  rss_mb: (v) => `${v} MB`,
};

export default function PdfTable({ data, metric = "ms" }) {
  if (!data) return null;
  const format = METRICS[metric];
  const rows = Object.keys(WORKLOADS).filter((w) => data[w]);
  const cols = ORDER.filter((rt) => rows.some((w) => data[w][rt]));
  return (
    <div className="mt-3 overflow-x-auto rounded-xl border border-zinc-200 bg-white dark:border-zinc-800 dark:bg-zinc-900">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500 dark:bg-zinc-800 dark:text-zinc-400">
          <tr>
            <th className="px-4 py-3 font-semibold">Workload</th>
            {cols.map((rt) => (
              <th className="px-4 py-3 text-right font-semibold">
                {LABELS[rt] || rt}
                <div className="font-mono text-[10px] normal-case tracking-normal text-zinc-400">{LIBRARY[rt]}</div>
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100 dark:divide-zinc-800">
          {rows.map((w) => {
            const best = Math.min(...cols.map((rt) => data[w][rt]?.[metric] ?? Infinity));
            return (
              <tr>
                <td className="px-4 py-3 text-zinc-900 dark:text-zinc-100">{WORKLOADS[w]}</td>
                {cols.map((rt) => {
                  const value = data[w][rt]?.[metric];
                  const missing = value == null;
                  const tone = missing
                    ? "text-zinc-400"
                    : value === best
                      ? "font-semibold text-emerald-600 dark:text-emerald-400"
                      : "text-zinc-700 dark:text-zinc-300";
                  return (
                    <td className={`px-4 py-3 text-right font-mono tabular-nums ${tone}`}>
                      {missing ? "n/a" : format(value)}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
