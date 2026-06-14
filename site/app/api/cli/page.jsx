import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const USAGE = `esrun <file>             Run a JavaScript module file
esrun -e <code>          Run an inline module snippet
esrun -t, --timeout <ms> Stop execution after <ms> ms (watchdog)
esrun upgrade            Update esrun to the latest release
esrun types              Print the runtime: TypeScript definitions
esrun -h, --help         Show this help
esrun -v, --version      Show the version`;

const RUN = `# Run a module file
esrun app.js

# Inline snippet (top-level await works)
esrun -e "console.log(await Promise.resolve(42))"

# Pass arguments through to the script (read via runtime:process)
esrun app.js build --watch

# Stop a runaway script after 500ms
esrun -t 500 app.js`;

const options = [
  {
    flag: "<file>",
    desc: "Path to a JavaScript ES module to run. Resolved as a local file (relative/absolute path or file: URL).",
  },
  {
    flag: "-e, --eval <code>",
    desc: "Run an inline module snippet instead of a file. Everything after <code> is passed to the script as arguments.",
  },
  {
    flag: "-t, --timeout <ms>",
    desc: "Watchdog: stop execution after <ms> milliseconds. Useful for bounding untrusted or long-running scripts.",
  },
  {
    flag: "upgrade",
    desc: "Download the latest release for your platform, verify its checksum, and replace the running binary in place.",
  },
  {
    flag: "types",
    desc: "Print the runtime: TypeScript definitions to stdout (esrun types > esrun.d.ts) — version-matched and offline. Also published as @opentf/esrun-types.",
  },
  {
    flag: "-h, --help",
    desc: "Print usage and exit.",
  },
  {
    flag: "-v, --version",
    desc: "Print the esrun version and exit.",
  },
];

export default function CliDoc() {
  return (
    <ApiShell active="/api/cli">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        esrun CLI
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The standalone <code className="font-mono">esrun</code> binary runs a
        JavaScript ES module file (or an inline snippet) end to end. Inputs run
        as modules — <code className="font-mono">import</code>/
        <code className="font-mono">export</code> and top-level{" "}
        <code className="font-mono">await</code> work.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Usage</h2>
      <div className="mt-4">
        <CodeBlock code={USAGE} title="esrun --help" lang="text" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Options</h2>
      <div className="mt-5 space-y-4">
        {options.map((o) => (
          <div className="rounded-xl border border-zinc-200 p-5">
            <code className="font-mono text-[15px] font-semibold text-zinc-900">
              {o.flag}
            </code>
            <p className="mt-2 text-sm leading-relaxed text-zinc-600">
              {o.desc}
            </p>
          </div>
        ))}
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Examples</h2>
      <div className="mt-4">
        <CodeBlock code={RUN} title="Terminal" lang="sh" />
      </div>

      <div className="mt-12 rounded-xl border border-zinc-200 bg-zinc-50 p-5 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Arguments.</strong> Anything after the
        file (or after the <code className="font-mono">-e</code> code) is the
        script's own argument list, readable as{" "}
        <a href="/api/process" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">args</code> from runtime:process
        </a>
        . The runtime binary and the script path are excluded.
      </div>
    </ApiShell>
  );
}
