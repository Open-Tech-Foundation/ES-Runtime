import DocsShell from "../../../components/DocsShell.jsx";
import StatusIcon from "../../../components/StatusIcon.jsx";

const nonGoals = [
  {
    title: "No Node.js compatibility",
    body: "esrun is not a drop-in for Node.js. There is no node: builtin namespace, no Node.js globals (no process global, no Buffer, no require), and no npm lifecycle. Code targets Web standards plus the runtime: modules, not the Node.js API.",
  },
  {
    title: "No CommonJS",
    body: "Modules are real ES Modules only. There is no require(), no module.exports, and no CJS↔ESM interop. A CommonJS-only package is rejected with a clear error.",
  },
  {
    title: "No TypeScript",
    body: "esrun runs JavaScript. It does not strip or compile types — transpile TypeScript ahead of time with your own toolchain and run the emitted JS.",
  },
  {
    title: "No JSX",
    body: "JSX is not a JavaScript standard; esrun does not transform it. Compile it ahead of time if you need it.",
  },
  {
    title: "No JSON module imports",
    body: 'import data from "./x.json" is not supported. Read and JSON.parse() the file through a runtime: API instead. (Import attributes are not implemented.)',
  },
  {
    title: "No package installer",
    body: "esrun resolves an existing node_modules tree but does not install anything. Use your package manager (npm, pnpm, bun) to populate dependencies.",
  },
  {
    title: "No bundler, linter, formatter, or test runner",
    body: "esrun is a runtime, not a toolchain. Bundling, linting, formatting, and testing are left to dedicated tools.",
  },
  {
    title: "No watch mode",
    body: "There is no built-in file watcher or auto-restart. Wrap esrun in your own watcher if you want one.",
  },
  {
    title: "No FFI or native addons",
    body: "There is no foreign-function interface and no native addon ABI. The host extends the runtime through injected providers and capability-gated ops, in Rust.",
  },
  {
    title: "No Workers (yet)",
    body: "Web Workers are not exposed. Multi-isolate execution is the goal of the embeddable VM layer (Layer B, the “es-vm”), where the host owns isolate lifecycle — not a Worker global in Layer A.",
  },
];

export default function ScopeDoc() {
  return (
    <DocsShell active="/docs/scope">
      <p className="text-sm font-medium text-brand-600">Getting started</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Scope &amp; non-goals
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun is a secure, standards-based runtime for running server-side
        JavaScript — and nothing more. Its scope is deliberately narrow. Knowing
        what it does <em>not</em> do is as important as knowing what it does.
      </p>

      <div className="mt-8 rounded-xl border border-brand-200 bg-brand-50 p-5 text-sm leading-relaxed text-brand-900">
        <strong>In scope:</strong> executing standard ES Modules on V8 with a
        Web-standard API surface, deny-by-default capabilities for embedders, a
        sandboxed module system, and host functionality exposed through{" "}
        <code className="rounded bg-white/60 px-1.5 py-0.5">runtime:</code>{" "}
        modules. esrun ships as an embeddable Rust library and a standalone CLI.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Non-goals</h2>
      <p className="mt-3 text-zinc-600">
        These are explicit, durable boundaries — not missing features awaiting
        implementation.
      </p>
      <div className="mt-6 space-y-4">
        {nonGoals.map((g) => (
          <div className="rounded-xl border border-zinc-200 p-5">
            <h3 className="flex items-start gap-2 text-base font-semibold text-zinc-900">
              <StatusIcon status="no" className="mt-0.5 inline-block h-[18px] w-[18px] shrink-0 text-rose-500" />
              <span>{g.title}</span>
            </h3>
            <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
              {g.body}
            </p>
          </div>
        ))}
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Two layers, one core
      </h2>
      <p className="mt-3 text-zinc-600">
        esrun is <strong>Layer A</strong>: a single driven runtime that the host
        ticks. <strong>Layer B</strong> — the embeddable “es-vm” — is the path to
        multiple isolates under host control, and is where concurrency
        primitives like Workers belong. Both ship from the same core; the
        Web-standard surface and capability model are shared.
      </p>
    </DocsShell>
  );
}
