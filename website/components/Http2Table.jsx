// HTTP/1.1 vs HTTP/2 table for the Benchmarks page — req/sec (higher is better).
// Two client shapes per runtime, because an HTTP/2 number in isolation says
// nothing: it is dominated by how many connections the client opened and how
// many streams it put on each. Used by app/docs/benchmarks/page.mdx.
//
import { LABELS, ORDER } from "../src/runtimes.js";

// The `gain` column is deliberately *not* rendered for every row the same way.
// Node and Bun serve cleartext h2 from `node:http2` while their HTTP/1.1 number
// comes from `node:http`/`Bun.serve`, so their ratio carries the gap between two
// implementations on top of the protocol change; those rows are marked and the
// caption says what the mark means. Comparing down a column is always fair.
const fmt = (n) => (n == null ? "n/a" : n.toLocaleString("en-US"));
const gain = (lo, hi) => (lo == null || hi == null ? "n/a" : `${(hi / lo).toFixed(2)}×`);

export default function Http2Table({ data }) {
  if (!data) return null;
  const rows = ORDER.filter((rt) => data[rt]);
  return (
    <div className="mt-3 overflow-x-auto rounded-xl border border-zinc-200">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
          <tr>
            <th className="px-4 py-3 font-semibold" rowSpan={2}>
              Runtime
            </th>
            <th className="px-4 py-3 text-center font-semibold" colSpan={3}>
              Wide — 50 conns × 1 stream
            </th>
            <th className="px-4 py-3 text-center font-semibold" colSpan={3}>
              Narrow — 1 conn × 50 streams
            </th>
          </tr>
          <tr>
            <th className="px-4 py-2 text-right font-semibold">HTTP/1.1</th>
            <th className="px-4 py-2 text-right font-semibold">HTTP/2</th>
            <th className="px-4 py-2 text-right font-semibold">Gain</th>
            <th className="px-4 py-2 text-right font-semibold">HTTP/1.1</th>
            <th className="px-4 py-2 text-right font-semibold">HTTP/2</th>
            <th className="px-4 py-2 text-right font-semibold">Gain</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100">
          {rows.map((rt) => {
            const d = data[rt];
            const mark = d.split_server ? "†" : "";
            return (
              <tr>
                <td className="px-4 py-3 font-mono text-zinc-900">{LABELS[rt] || rt}</td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-600">
                  {fmt(d.wide_h1)}
                </td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-600">
                  {fmt(d.wide_h2)}
                </td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-500">
                  {gain(d.wide_h1, d.wide_h2)}
                  {mark}
                </td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-600">
                  {fmt(d.narrow_h1)}
                </td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-600">
                  {fmt(d.narrow_h2)}
                </td>
                <td className="px-4 py-3 text-right font-mono tabular-nums text-zinc-500">
                  {gain(d.narrow_h1, d.narrow_h2)}
                  {mark}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
