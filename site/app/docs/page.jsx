import DocsShell from "../../components/DocsShell.jsx";
import CodeBlock from "../../components/CodeBlock.jsx";

const BUILD = `# Build the standalone CLI from source
git clone https://github.com/Open-Tech-Foundation/ES-Runtime
cd ES-Runtime
cargo build --release

# The binary is target/release/esrun`;

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
        built on V8 in Rust. It runs standard ES Modules with a deny-by-default
        capability model and a sandboxed module system, and ships both as an
        embeddable library and as a standalone CLI,{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">esrun</code>.
        It is <strong>not</strong> Node-compatible — see{" "}
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
        Build the CLI from source with a recent stable Rust toolchain:
      </p>
      <div className="mt-4">
        <CodeBlock code={BUILD} title="Terminal" lang="sh" />
      </div>

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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Next steps</h2>
      <div className="mt-4 grid gap-4 sm:grid-cols-2">
        <a
          href="/docs/modules"
          className="rounded-xl border border-zinc-200 p-5 transition-shadow hover:shadow-sm"
        >
          <div className="font-semibold text-zinc-900">Module system →</div>
          <p className="mt-1 text-sm text-zinc-600">
            Imports, dynamic import, node_modules, and the runtime: scheme.
          </p>
        </a>
        <a
          href="/docs/globals"
          className="rounded-xl border border-zinc-200 p-5 transition-shadow hover:shadow-sm"
        >
          <div className="font-semibold text-zinc-900">Global objects →</div>
          <p className="mt-1 text-sm text-zinc-600">
            The Web-standard globals available in the runtime.
          </p>
        </a>
        <a
          href="/api"
          className="rounded-xl border border-zinc-200 p-5 transition-shadow hover:shadow-sm"
        >
          <div className="font-semibold text-zinc-900">runtime:process →</div>
          <p className="mt-1 text-sm text-zinc-600">
            Environment, arguments, working directory, platform, and exit.
          </p>
        </a>
        <a
          href="/docs/benchmarks"
          className="rounded-xl border border-zinc-200 p-5 transition-shadow hover:shadow-sm"
        >
          <div className="font-semibold text-zinc-900">Benchmarks →</div>
          <p className="mt-1 text-sm text-zinc-600">
            Measured performance vs Node, Bun, and Deno.
          </p>
        </a>
      </div>
    </DocsShell>
  );
}
