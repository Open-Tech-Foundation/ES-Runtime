import DocsShell from "../../../components/DocsShell.jsx";

const caps = [
  { name: "Env", desc: "Read environment, arguments, cwd, platform; backs runtime:process." },
  { name: "FileRead", desc: "Read files within the configured root jail." },
  { name: "FileWrite", desc: "Write files within the configured root jail." },
  { name: "Net", desc: "Open outbound network connections (runtime:net connect, fetch, WebSocket)." },
  { name: "NetListen", desc: "Bind a listening socket and accept connections (runtime:net listen, runtime:http / runtime:websocket serve). Server-side TLS terminates under this capability — the cert/key are passed inline, so no extra grant is needed." },
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
        reaching a different module path. A denied capability will instantly
        throw a standard <code>DOMException</code> with the <code>NotAllowedError</code> name.
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

      <div className="mt-10 mb-8 rounded-2xl bg-zinc-950 p-8 shadow-inner border border-zinc-800">
        <div className="flex flex-col items-center gap-6">
          <div className="relative flex w-full max-w-lg flex-col sm:flex-row items-center justify-between rounded-lg border-2 border-dashed border-rose-500/50 bg-rose-500/10 p-6 text-center gap-4 sm:gap-0">
            <span className="absolute -top-3 left-4 bg-zinc-950 px-2 text-xs font-semibold uppercase tracking-wider text-rose-400">
              JS Runtime (Untrusted)
            </span>
            <div className="text-sm font-medium text-zinc-300 text-left">
              Your Code
              <div className="mt-1 rounded bg-zinc-900 px-2 py-1">
                <code className="text-rose-300">file("../../etc/passwd")</code>
              </div>
            </div>
            <div className="hidden sm:block text-2xl text-zinc-500">→</div>
            <div className="block sm:hidden text-2xl text-zinc-500">↓</div>
            <div className="flex flex-col gap-2 text-xs w-full sm:w-auto">
              <div className="rounded border border-brand-500/30 bg-brand-500/20 px-3 py-1.5 font-mono text-brand-300">
                Capability: FileRead
              </div>
              <div className="rounded border border-emerald-500/30 bg-emerald-500/20 px-3 py-1.5 font-mono text-emerald-300">
                Root Jail Check
              </div>
            </div>
          </div>
          
          <div className="h-6 border-l-2 border-dashed border-zinc-700"></div>
          
          <div className="w-full max-w-lg rounded-lg border border-zinc-700 bg-zinc-900 p-6 text-center shadow-lg">
            <span className="mb-3 block text-xs font-semibold uppercase tracking-wider text-emerald-400">
              Rust Host (Trusted)
            </span>
            <div className="text-sm font-medium text-zinc-400">
              Path canonicalized to: <code className="text-zinc-200">/etc/passwd</code>
            </div>
            <div className="mt-4 rounded bg-rose-500/20 px-4 py-2 text-sm font-semibold text-rose-400 border border-rose-500/30">
              DENIED: Path escapes project root (/home/user/project)
            </div>
          </div>
        </div>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Environment files &amp; secret masking
      </h2>
      <p className="mt-3 text-zinc-600">
        A <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.env</code>{" "}
        file is loaded only when you pass{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">--env-file</code>{" "}
        — there is no auto-discovery, so nothing on disk reaches the guest's
        environment unless you ask for it. The OS environment wins on a conflict
        by default (a checked-in file can't clobber production config);{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">--env-override</code>{" "}
        opts into letting the file win. The real process environment is never
        mutated.
      </p>
      <p className="mt-3 text-zinc-600">
        Env values whose key ends in{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">*_SECRET(S)</code>{" "}
        or{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">*_PASSWORD(S)</code>{" "}
        are exposed as a{" "}
        <a href="/api/process" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">Secret</code>
        </a>{" "}
        that renders as <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">[redacted]</code>{" "}
        in logs, string coercion, and JSON — readable only via{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">unmask()</code>.
        This prevents <em>accidental</em> leakage to logs; it is not a barrier
        against hostile guest code, which can unmask the value itself.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Remote modules disabled
      </h2>
      <p className="mt-3 text-zinc-600">
        esrun intentionally drops support for downloading modules dynamically over 
        the network (e.g., <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">import "https://..."</code>). 
        This mitigates entire classes of supply-chain attacks and runtime hijacking 
        because every piece of executed code must explicitly reside within the secure 
        local filesystem root, greatly improving predictability and security.
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
