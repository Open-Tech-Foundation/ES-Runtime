// The runtimes measured on the Benchmarks page, with their versions (from the
// generated bench data). Used by app/docs/benchmarks/page.mdx.
import bench from "../src/benchmarks.js";

const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };

export default function RuntimeVersions() {
  const versions = Object.keys(bench.runtimes).map((k) => ({
    k: LABELS[k] || k,
    v: bench.runtimes[k],
  }));
  return (
    <ul className="mt-2 grid gap-1 font-mono text-[12px] sm:grid-cols-2">
      {versions.map((r) => (
        <li>
          <span className="text-brand-700">{r.k}</span>{" "}
          <span className="text-zinc-500">{r.v}</span>
        </li>
      ))}
    </ul>
  );
}
