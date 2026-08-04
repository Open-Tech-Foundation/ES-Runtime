// What each runtime does when a script asks for more memory than it can have.
// Fed by bench/memory-safety.sh via bench/gen-bench-data.sh (`memory_safety`).
//
// Not a speed table: the only question is whether the runtime refused in a way
// the guest could survive. "graceful" means JS got a catchable error or the
// process exited cleanly; "crash" means it took a signal and the guest never got
// a say, which in a server is the difference between one failed request and a
// dropped process.
//
// NOTE: the @opentf/web compiler rewrites `.map()` into a reactive list helper,
// so non-render computations must use plain loops.
import bench from "../src/benchmarks.js";

const ORDER = ["esrun", "bun", "node", "deno", "llrt"];
const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };
const ROWS = {
  mem_nested_json: "200k-deep nested array → JSON.stringify",
  mem_large_string: "String doubled past the engine maximum",
  mem_promise_leak: "10M chained .then()",
};

// Signal numbers worth naming; anything else falls back to the raw number.
const SIGNALS = { 4: "SIGILL", 6: "SIGABRT", 9: "SIGKILL", 11: "SIGSEGV" };

function verdict(raw) {
  if (raw === "graceful") return { text: "graceful", tone: "ok" };
  if (raw === "timeout") return { text: "timeout", tone: "warn" };
  if (typeof raw === "string" && raw.startsWith("crash:")) {
    const n = Number(raw.slice(6));
    return { text: SIGNALS[n] || `signal ${n}`, tone: "bad" };
  }
  if (typeof raw === "string" && raw.startsWith("exit:")) {
    return { text: `exit ${raw.slice(5)}`, tone: "warn" };
  }
  return { text: "—", tone: "none" };
}

const TONE = {
  ok: "text-emerald-700 dark:text-emerald-400",
  warn: "text-amber-600 dark:text-amber-400",
  bad: "font-semibold text-red-600 dark:text-red-400",
  none: "text-zinc-400",
};

export default function MemorySafetyTable() {
  const data = bench.memory_safety;
  if (!data) return null;

  const runtimes = [];
  for (const rt of ORDER) if (bench.runtimes[rt]) runtimes.push(rt);

  const rowKeys = [];
  for (const k of Object.keys(ROWS)) if (data[k]) rowKeys.push(k);
  if (rowKeys.length === 0) return null;

  return (
    <div className="overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead>
          <tr className="border-b border-zinc-200 dark:border-zinc-700">
            <th className="px-3 py-2 font-medium">Scenario</th>
            {runtimes.map((rt) => (
              <th className="px-3 py-2 text-right font-medium">{LABELS[rt]}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rowKeys.map((k) => (
            <tr className="border-b border-zinc-100 dark:border-zinc-800">
              <td className="px-3 py-2 text-zinc-600 dark:text-zinc-400">{ROWS[k]}</td>
              {runtimes.map((rt) => (
                <td className={"px-3 py-2 text-right " + TONE[verdict(data[k][rt]).tone]}>
                  {verdict(data[k][rt]).text}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
