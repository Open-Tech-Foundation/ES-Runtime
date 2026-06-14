import DocsShell from "../../../components/DocsShell.jsx";
import StatusIcon from "../../../components/StatusIcon.jsx";

// status: "yes" | "partial" | "no"
const rows = [
  { f: "ES Modules", esrun: "yes", node: "yes", bun: "yes", deno: "yes" },
  { f: "CommonJS (require)", esrun: "no", node: "yes", bun: "yes", deno: "partial" },
  { f: "TypeScript (built-in)", esrun: "no", node: "yes", bun: "yes", deno: "yes" },
  { f: "JSX (built-in)", esrun: "no", node: "no", bun: "yes", deno: "yes" },
  { f: "JSON module imports", esrun: "no", node: "yes", bun: "yes", deno: "yes" },
  { f: "Web APIs (fetch/URL/streams/WebCrypto)", esrun: "yes", node: "yes", bun: "yes", deno: "yes" },
  { f: "Node compatibility (node: builtins)", esrun: "no", node: "yes", bun: "yes", deno: "partial" },
  { f: "Capability sandbox (deny by default)", esrun: "yes", node: "partial", bun: "no", deno: "yes" },
  { f: "Workers / multi-thread", esrun: "no", node: "yes", bun: "yes", deno: "yes" },
  { f: "FFI (dlopen)", esrun: "no", node: "no", bun: "yes", deno: "yes" },
  { f: "Native addons (N-API)", esrun: "no", node: "yes", bun: "yes", deno: "yes" },
  { f: "Package installer", esrun: "no", node: "yes", bun: "yes", deno: "yes" },
  { f: "Bundler / test runner", esrun: "no", node: "partial", bun: "yes", deno: "yes" },
  { f: "Embeddable as a library", esrun: "yes", node: "no", bun: "no", deno: "partial" },
];

const cols = ["esrun", "node", "bun", "deno"];
const colLabel = { esrun: "esrun", node: "Node", bun: "Bun", deno: "Deno" };

export default function ComparisonDoc() {
  return (
    <DocsShell active="/docs/comparison">
      <p className="text-sm font-medium text-brand-600">Comparisons</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        esrun vs Node · Bun · Deno
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Node, Bun, and Deno are general-purpose runtimes with broad toolchains
        and (varying degrees of) Node compatibility. esrun is intentionally
        smaller: a sandboxed, standards-only execution core you embed or run
        directly. The table makes the trade-offs explicit.
      </p>

      <div className="mt-8 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <thead className="bg-zinc-50 text-zinc-600">
            <tr>
              <th className="px-4 py-3 font-medium">Capability</th>
              {cols.map((c) => (
                <th
                  className={
                    "px-3 py-3 text-center font-mono text-[13px] font-semibold " +
                    (c === "esrun" ? "text-brand-700" : "text-zinc-700")
                  }
                >
                  {colLabel[c]}
                </th>
              ))}
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-100">
            {rows.map((r) => (
              <tr className="hover:bg-zinc-50/60">
                <td className="px-4 py-2.5 text-zinc-700">{r.f}</td>
                {cols.map((c) => (
                  <td
                    className={
                      "px-3 py-2.5 text-center " +
                      (c === "esrun" ? "bg-brand-50/40" : "")
                    }
                  >
                    <StatusIcon status={r[c]} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="mt-4 flex flex-wrap items-center gap-5 text-xs text-zinc-500">
        <span className="inline-flex items-center gap-1.5">
          <StatusIcon status="yes" /> Supported
        </span>
        <span className="inline-flex items-center gap-1.5">
          <StatusIcon status="partial" /> Partial / flagged / experimental
        </span>
        <span className="inline-flex items-center gap-1.5">
          <StatusIcon status="no" /> Not supported
        </span>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Where the API surface differs
      </h2>
      <ul className="mt-4 space-y-3 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">No Node API.</strong> Where Node,
            Bun, and Deno all expose <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">node:fs</code>,{" "}
            <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Buffer</code>, and a global{" "}
            <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">process</code>, esrun exposes host
            functionality only through capability-gated{" "}
            <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:</code> modules.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Globals are not host APIs.</strong>{" "}
            Like Deno, esrun keeps the global scope close to the Web platform —
            but it does not put filesystem or process access on a global either.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Sandbox is the default.</strong>{" "}
            Deno is permission-prompted and Node has an experimental permission
            model; esrun is deny-by-default with a filesystem root jail that is
            always on.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">It embeds.</strong> esrun is a Rust
            library with a driven event loop and no owned thread, designed to run
            inside a host application — a use case the others do not target.
          </span>
        </li>
      </ul>

      <p className="mt-8 text-sm text-zinc-500">
        Status reflects general availability at the time of writing and is a
        summary, not an exhaustive audit. See{" "}
        <a href="/docs/benchmarks" className="font-medium text-brand-600 hover:text-brand-700">
          Benchmarks
        </a>{" "}
        for measured performance.
      </p>
    </DocsShell>
  );
}
