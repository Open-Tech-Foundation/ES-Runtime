import ApiShell from "../../components/ApiShell.jsx";
import CodeBlock from "../../components/CodeBlock.jsx";

const IMPORT = `import { env, args } from "runtime:process";`;

const modules = [
  { name: "runtime:process", status: "Available", cap: "Env", href: "/api/process" },
  { name: "runtime:path", status: "Available", cap: "Env", href: "/api/path" },
  { name: "runtime:fs", status: "Available", cap: "FileRead / FileWrite", href: "/api/fs" },
  { name: "runtime:net", status: "Available", cap: "Net / NetListen", href: "/api/net" },
  { name: "runtime:http", status: "Planned", cap: "Net", href: null },
];

export default function ApiOverview() {
  return (
    <ApiShell active="/api">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Overview
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES Runtime is ESM-only; the embeddable library is deny-by-default (the
        esrun CLI grants all capabilities). Host functionality is exposed as ES
        modules under the{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 font-mono text-[0.9em]">
          runtime:
        </code>{" "}
        scheme — never as ambient globals — and every operation is gated on an
        explicit capability.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        The <span className="font-mono">runtime:</span> scheme
      </h2>
      <p className="mt-3 text-zinc-600">
        Built-in modules are imported with a <code className="font-mono">runtime:</code>{" "}
        specifier. They are served from a baked, in-binary registry before any
        injected loader runs, and never touch the filesystem. The security
        boundary is the <strong>op</strong>, not the module: importing always
        succeeds, but an operation throws unless its capability has been granted.
      </p>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="import" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Built-in modules
      </h2>
      <div className="mt-5 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
            <tr>
              <th className="px-4 py-3 font-semibold">Module</th>
              <th className="px-4 py-3 font-semibold">Status</th>
              <th className="px-4 py-3 font-semibold">Capability</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-100">
            {modules.map((m) => (
              <tr>
                <td className="px-4 py-3 font-mono text-zinc-900">
                  {m.href ? (
                    <a href={m.href} className="text-brand-600 hover:text-brand-700">
                      {m.name}
                    </a>
                  ) : (
                    m.name
                  )}
                </td>
                <td className="px-4 py-3">
                  <span
                    className={
                      "rounded-full px-2.5 py-0.5 text-xs font-medium " +
                      (m.status === "Available"
                        ? "bg-emerald-50 text-emerald-700"
                        : "bg-zinc-100 text-zinc-500")
                    }
                  >
                    {m.status}
                  </span>
                </td>
                <td className="px-4 py-3 font-mono text-zinc-600">{m.cap}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </ApiShell>
  );
}
