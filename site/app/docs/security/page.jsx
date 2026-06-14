import DocsShell from "../../../components/DocsShell.jsx";

const caps = [
  { name: "Env", desc: "Read environment, arguments, cwd, platform; backs runtime:process." },
  { name: "FileRead", desc: "Read files within the configured root jail." },
  { name: "FileWrite", desc: "Write files within the configured root jail." },
  { name: "Net", desc: "Open outbound network connections." },
  { name: "HrTime", desc: "Access high-resolution timing." },
];

export default function SecurityDoc() {
  return (
    <DocsShell active="/docs/security">
      <p className="text-sm font-medium text-brand-600">Concepts</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Security model
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES Runtime ships in two forms, and they differ on what code can reach by
        default:
      </p>
      <ul className="mt-4 space-y-2 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Embeddable library — deny-by-default.</strong>{" "}
            A runtime the host creates can compute, but cannot reach the host
            environment, filesystem, or network until the host grants a
            capability for it.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">The <code className="font-mono">esrun</code> CLI — unrestricted.</strong>{" "}
            The standalone binary grants all capabilities so scripts run without
            setup. It is not deny-by-default.
          </span>
        </li>
      </ul>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Capabilities</h2>
      <p className="mt-3 text-zinc-600">
        Every host operation declares the capability it requires. The check
        lives on the native op, not in JavaScript, so it cannot be bypassed by
        reaching a different module path.
      </p>
      <div className="mt-5 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <thead className="bg-zinc-50 text-zinc-500">
            <tr>
              <th className="px-4 py-2.5 font-medium">Capability</th>
              <th className="px-4 py-2.5 font-medium">Grants</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-100">
            {caps.map((c) => (
              <tr>
                <td className="px-4 py-2.5 font-mono text-[13px] text-brand-700">
                  {c.name}
                </td>
                <td className="px-4 py-2.5 text-zinc-600">{c.desc}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        The filesystem root jail
      </h2>
      <p className="mt-3 text-zinc-600">
        Filesystem access — including module resolution — is confined to a
        single project root. Paths are canonicalized to their real location
        before the check, so a symlink cannot be used to escape the jail. This
        is on by default and is not currently optional.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Engine confinement
      </h2>
      <p className="mt-3 text-zinc-600">
        All V8 contact is contained in a single engine crate; the rest of the
        runtime never names a V8 type. This keeps the trusted surface small and
        auditable, and lets the host drive the event loop without surrendering
        control of its own thread.
      </p>
    </DocsShell>
  );
}
