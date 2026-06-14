import DocsShell from "../../components/DocsShell.jsx";
import CodeBlock from "../../components/CodeBlock.jsx";
import InstallBox from "../../components/InstallBox.jsx";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

const RUN = `# Run a module file
esrun app.mjs

# Evaluate an inline snippet (top-level await is supported)
esrun -e "console.log(await Promise.resolve(42))"

# Pass arguments through to the script
esrun app.mjs --name Ada`;

export default function DocsOverview() {
  return (
    <DocsShell active="/docs">
      <p className="text-sm font-medium text-brand-600">Getting started</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Overview
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun is a secure, standards-based JavaScript runtime for the server,
        built on V8 in Rust. It runs standard ES Modules with a sandboxed module
        system; its embeddable library is deny-by-default, granting host
        capabilities only when the host asks.
      </p>

      <div className="mt-6 rounded-xl border border-brand-200 bg-brand-50 p-5 leading-relaxed text-brand-900">
        Ships in two shapes from one core: an{" "}
        <strong>embeddable library</strong> and a{" "}
        <strong>standalone CLI</strong> (
        <code className="rounded bg-white/70 px-1.5 py-0.5 text-[13px]">esrun</code>
        ).
      </div>

      <p className="mt-4 text-zinc-600">
        It is <strong>not</strong> Node.js-compatible — see{" "}
        <a href="/docs/scope" className="font-medium text-brand-600 hover:text-brand-700">
          Scope &amp; non-goals
        </a>{" "}
        and the{" "}
        <a href="/docs/comparison" className="font-medium text-brand-600 hover:text-brand-700">
          runtime comparison
        </a>
        .
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Install</h2>
      <p className="mt-3 text-zinc-600">
        Download a prebuilt, checksum-verified binary for your platform:
      </p>
      <div className="mt-4">
        <InstallBox />
      </div>
      <p className="mt-3 text-sm text-zinc-500">
        Prefer to build from source? See the{" "}
        <a
          href={GITHUB}
          target="_blank"
          rel="noreferrer"
          className="font-medium text-brand-600 hover:text-brand-700"
        >
          README
        </a>
        .
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Run a script</h2>
      <div className="mt-4">
        <CodeBlock code={RUN} title="Terminal" lang="sh" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Principles
      </h2>
      <ul className="mt-4 space-y-3 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">ESM only.</strong> There is no
            CommonJS interop. Modules are real ES Modules and packages must
            expose an ESM entry point.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Capabilities are explicit.</strong>{" "}
            Host powers — environment, filesystem, network — are granted one at a
            time. Nothing is ambient.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Standard surface.</strong> Host
            functionality is exposed through <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:</code> module
            imports rather than magic globals.
          </span>
        </li>
      </ul>
    </DocsShell>
  );
}
