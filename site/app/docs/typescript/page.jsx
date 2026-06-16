import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const TSCONFIG = `{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "typeRoots": ["node_modules/@types", "node_modules/@opentf"],
    "types": ["esrun"]
  },
  "include": ["**/*.ts"]
}`;

export default function TypescriptDoc() {
  return (
    <DocsShell active="/docs/typescript">
      <p className="text-sm font-medium text-brand-600">Getting started</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        TypeScript setup
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun does not execute TypeScript natively, nor does it inject ambient globals like <code className="font-mono bg-zinc-100 px-1 rounded">Bun</code> or <code className="font-mono bg-zinc-100 px-1 rounded">process</code>. But you can get full editor autocompletion and type checking for the <code className="font-mono bg-zinc-100 px-1 rounded">runtime:*</code> modules in one command.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">One-command setup</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono bg-zinc-100 px-1 rounded">esrun types --install</code> writes the bundled definitions into <code className="font-mono bg-zinc-100 px-1 rounded">node_modules/@opentf/esrun</code> (as a type package, so your project tree stays clean) and wires them into your <code className="font-mono bg-zinc-100 px-1 rounded">tsconfig.json</code> — creating one if you don't have it, or merging into an existing one without touching your other settings.
      </p>
      <div className="mt-4">
        <CodeBlock code="esrun types --install" lang="sh" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">What it configures</h2>
      <p className="mt-3 text-zinc-600">
        The types are registered through <code className="font-mono bg-zinc-100 px-1 rounded">typeRoots</code> + <code className="font-mono bg-zinc-100 px-1 rounded">types</code> — the form editors and language servers load globally, so the <code className="font-mono bg-zinc-100 px-1 rounded">runtime:*</code> modules resolve everywhere, not just per-file:
      </p>
      <div className="mt-4">
        <CodeBlock code={TSCONFIG} title="tsconfig.json" lang="json" />
      </div>
      <div className="mt-4 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Manual alternative:</strong> if you'd rather not let the CLI edit your config, run <code className="font-mono">esrun types &gt; runtime.d.ts</code> and add that file to your <code className="font-mono">include</code>.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Use it</h2>
      <p className="mt-3 text-zinc-600">
        Your editor now provides intellisense, inline docs, and type checking for every built-in module:
      </p>
      <div className="mt-4">
        <CodeBlock code={`import { file } from "runtime:fs";

// Your IDE knows \`text()\` returns a Promise<string>
const data = await file("./config.json").text();`} title="app.ts" lang="ts" />
      </div>
    </DocsShell>
  );
}
