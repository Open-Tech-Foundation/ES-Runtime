import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const NODE_PROCESS = `// Node.js
const port = process.env.PORT;
const args = process.argv.slice(2);
const cwd = process.cwd();

// ESRun
import { env, args, cwd } from "runtime:process";

const port = env.PORT;
const args = args;
const dir = cwd();`;

const NODE_FS = `// Node.js
import { readFileSync, writeFileSync } from "node:fs";
const data = readFileSync("./data.txt", "utf8");
writeFileSync("./data.txt", "done");

// ESRun
import { file, write } from "runtime:fs";
const data = await file("./data.txt").text();
await write("./data.txt", "done");`;

const BUN_FILE = `// Bun
const data = await Bun.file("./data.txt").text();
await Bun.write("./data.txt", "done");

// ESRun
import { file, write } from "runtime:fs";
const data = await file("./data.txt").text();
await write("./data.txt", "done");`;

const DENO_FILE = `// Deno
const data = await Deno.readTextFile("./data.txt");
await Deno.writeTextFile("./data.txt", "done");

// ESRun
import { file, write } from "runtime:fs";
const data = await file("./data.txt").text();
await write("./data.txt", "done");`;

export default function MigrationDoc() {
  return (
    <DocsShell active="/docs/migration">
      <p className="text-sm font-medium text-brand-600">Getting started</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Migration Guide
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        A comprehensive guide to moving from Node.js, Bun, or Deno to esrun.
      </p>

      <div className="mt-6 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Core Principle:</strong> No ambient globals. Unlike other runtimes, esrun does not expose global <code className="font-mono">process</code>, <code className="font-mono">Bun</code>, or <code className="font-mono">Deno</code> objects. You explicitly import what you need from the <code className="font-mono">runtime:</code> scheme.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">From Node.js</h2>
      <p className="mt-3 text-zinc-600">
        esrun uses ES modules exclusively. CommonJS is not supported. Built-in modules like <code className="font-mono">node:fs</code> or <code className="font-mono">node:path</code> are replaced by secure, scoped equivalents under the <code className="font-mono">runtime:</code> prefix.
      </p>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Process & Env globals</h3>
      <p className="mt-3 text-zinc-600">
        In Node.js, <code className="font-mono">process</code> is a global object. In esrun, you must explicitly import environment variables and arguments from <code className="font-mono">runtime:process</code>.
      </p>
      <div className="mt-4">
        <CodeBlock code={NODE_PROCESS} title="process" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">File System</h3>
      <p className="mt-3 text-zinc-600">
        esrun moves away from synchronous file operations and callback APIs, embracing a modern, Promise-based <code className="font-mono">Blob</code>-like interface.
      </p>
      <div className="mt-4">
        <CodeBlock code={NODE_FS} title="fs" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">From Bun</h2>
      <p className="mt-3 text-zinc-600">
        Bun and esrun share similarities in embracing modern Web APIs and the <code className="font-mono">Blob</code> file structure. However, esrun requires explicit imports rather than relying on the <code className="font-mono">Bun</code> global object.
      </p>
      <div className="mt-4">
        <CodeBlock code={BUN_FILE} title="file" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">From Deno</h2>
      <p className="mt-3 text-zinc-600">
        Similar to Deno, esrun focuses on standard web APIs and a secure capability model. Instead of relying on the <code className="font-mono">Deno.*</code> namespace, esrun exposes explicit <code className="font-mono">runtime:</code> modules. Note that esrun uses standard <code className="font-mono">package.json</code> and <code className="font-mono">node_modules</code> resolution instead of URL imports for packages.
      </p>
      <div className="mt-4">
        <CodeBlock code={DENO_FILE} title="file" lang="js" />
      </div>
    </DocsShell>
  );
}
