// Burst throughput against sustained throughput, for the same Hono server.
//
// The req/s chart above is a burst: a fixed number of requests fired at a
// freshly started process. This is the same server held under load for a fixed
// window, so the heap is full and the collector has been working the whole time.
// The change column is the part that matters — a runtime that starts fast and
// then gives some back is a different proposition from one that holds.
//
// Fed by bench/rps.sh via `SECTIONS=rps_sustained bench/gen-bench-data.sh`,
// published under `results_rps.hono_sustained`.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so non-render computations must use plain loops.
import bench from "../src/benchmarks.js";

const ORDER = ["esrun", "bun", "node", "deno"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno" };

function fmt(v) {
  return typeof v === "number" ? (v / 1000).toFixed(1) + "k" : "n/a";
}

// Signed, because the direction is the whole point and an unsigned "3%" reads
// as a gain either way.
function change(burst, held) {
  if (typeof burst !== "number" || typeof held !== "number" || !burst) return null;
  return (100 * (held - burst)) / burst;
}

export default function SustainedRpsTable() {
  const burst = bench.results_rps?.hono;
  const held = bench.results_rps?.hono_sustained;
  if (!burst || !held) return null;

  const windowLabel = bench.rps_method?.hono_sustained?.duration || "held";

  const runtimes = [];
  for (const rt of ORDER) if (bench.runtimes[rt]) runtimes.push(rt);

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead>
          <tr className="border-b border-zinc-200 dark:border-zinc-700">
            <th className="px-3 py-2 font-medium">Runtime</th>
            <th className="px-3 py-2 text-right font-medium">Burst req/s</th>
            <th className="px-3 py-2 text-right font-medium">{windowLabel} req/s</th>
            <th className="px-3 py-2 text-right font-medium">Change</th>
          </tr>
        </thead>
        <tbody>
          {runtimes.map((rt) => {
            const pct = change(burst[rt], held[rt]);
            return (
              <tr className="border-b border-zinc-100 dark:border-zinc-800">
                <td className="px-3 py-2">{LABELS[rt]}</td>
                <td className="px-3 py-2 text-right tabular-nums text-zinc-500">
                  {fmt(burst[rt])}
                </td>
                <td className="px-3 py-2 text-right tabular-nums text-zinc-500">
                  {fmt(held[rt])}
                </td>
                <td
                  className={
                    pct === null
                      ? "px-3 py-2 text-right text-zinc-400"
                      : pct < -5
                        ? "px-3 py-2 text-right font-semibold tabular-nums text-amber-600 dark:text-amber-400"
                        : "px-3 py-2 text-right tabular-nums text-emerald-700 dark:text-emerald-400"
                  }
                >
                  {pct === null ? "—" : (pct >= 0 ? "+" : "") + pct.toFixed(1) + "%"}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
