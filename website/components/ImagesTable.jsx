// Image pipeline table for the Benchmarks page: wall ms (lower is better), peak
// RSS, and the size of one output, because two encoders are only comparable
// when they produce files of similar size. Data comes from
// bench/images/run.sh via gen-bench-data.sh. Used by app/docs/benchmarks/page.mdx.

import { LABELS, ORDER } from "../src/runtimes.js";

const WORKLOADS = {
  jpeg_webp: "12 MP JPEG → 400 px WebP",
  jpeg_jpeg: "12 MP JPEG → 400 px JPEG",
  png_jpeg: "1280×800 PNG → 320 px JPEG",
  jpeg_avif: "12 MP JPEG → 400 px AVIF",
};

// Each runtime's own image API; Node.js has none, so it runs sharp.
const LIBRARY = { node: "sharp", deno: "createImageBitmap", bun: "Bun.Image", esrun: "runtime:images" };

// What a table shows. `best` is the cell to highlight: lowest time and memory;
// output size has no winner, since smaller is not better at a given quality.
const METRICS = {
  ms: { format: (v) => `${Math.round(v).toLocaleString("en-US")} ms`, best: true },
  bytes: { format: (v) => `${(v / 1024).toFixed(1)} KB`, best: false },
  rss_mb: { format: (v) => `${v} MB`, best: true },
};

export default function ImagesTable({ data, metric = "ms" }) {
  if (!data) return null;
  const { format, best: highlight } = METRICS[metric];
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
                  const win = !missing && highlight && value === best;
                  const tone = missing
                    ? "text-zinc-400"
                    : win
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
