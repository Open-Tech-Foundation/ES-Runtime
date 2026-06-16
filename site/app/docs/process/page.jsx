import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const PROCESS = `import { env, args, cwd } from "runtime:process";

console.log("Current directory:", cwd());
console.log("Arguments:", args);
console.log("HOME:", env.HOME);`;

export default function ProcessDoc() {
  return (
    <DocsShell active="/docs/process">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Process &amp; Env
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:process</code> module provides access to the current process environment, command-line arguments, and the working directory.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Usage</h2>
      <div className="mt-4">
        <CodeBlock code={PROCESS} title="app.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Real-world Example</h2>
      <p className="mt-3 text-zinc-600">
        Loading config based on environment:
      </p>
      <div className="mt-4">
        <CodeBlock code={`import { env } from "runtime:process";
import { file } from "runtime:fs";

const isProd = env.NODE_ENV === "production";
const configPath = isProd ? "./config/prod.json" : "./config/dev.json";

const config = await file(configPath).json();
console.log("Loaded config:", config);`} title="config.js" lang="js" />
      </div>

      <p className="mt-12 text-sm text-zinc-500">
        For more details on available properties, view the{" "}
        <a href="/api/process" className="font-medium text-brand-600 hover:text-brand-700">
          API Reference
        </a>.
      </p>
    </DocsShell>
  );
}
