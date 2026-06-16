import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const TSCONFIG = `{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true
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
        esrun does not execute TypeScript natively, nor does it inject ambient globals like <code className="font-mono bg-zinc-100 px-1 rounded">Bun</code> or <code className="font-mono bg-zinc-100 px-1 rounded">process</code> into your environment. However, you can easily set up your IDE to provide full autocompletion for esrun's <code className="font-mono bg-zinc-100 px-1 rounded">runtime:*</code> modules.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">1. Generate types</h2>
      <p className="mt-3 text-zinc-600">
        The esrun binary ships with its own TypeScript definitions built-in. Write them to a <code className="font-mono bg-zinc-100 px-1 rounded">.d.ts</code> file in your project — TypeScript picks up any <code className="font-mono bg-zinc-100 px-1 rounded">.d.ts</code> automatically:
      </p>
      <div className="mt-4">
        <CodeBlock code="esrun types > esrun.d.ts" lang="sh" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">2. Configure tsconfig.json</h2>
      <p className="mt-3 text-zinc-600">
        A modern, web-standard <code className="font-mono bg-zinc-100 px-1 rounded">tsconfig.json</code>. The <code className="font-mono bg-zinc-100 px-1 rounded">include</code> glob matches your sources and the generated <code className="font-mono bg-zinc-100 px-1 rounded">esrun.d.ts</code> (<code className="font-mono bg-zinc-100 px-1 rounded">**/*.ts</code> covers <code className="font-mono bg-zinc-100 px-1 rounded">.d.ts</code> too):
      </p>
      <div className="mt-4">
        <CodeBlock code={TSCONFIG} title="tsconfig.json" lang="json" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">3. Import types seamlessly</h2>
      <p className="mt-3 text-zinc-600">
        Once configured, your IDE will automatically provide rich intellisense, inline documentation, and type checking for all esrun built-in modules.
      </p>
      <div className="mt-4">
        <CodeBlock code={`import { file } from "runtime:fs";

// Your IDE knows \`text()\` returns a Promise<string>
const data = await file("./config.json").text();`} title="app.js" lang="js" />
      </div>
    </DocsShell>
  );
}
