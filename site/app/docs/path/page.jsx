import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const PATH = `import { join, resolve, basename, dirname, extname } from "runtime:path";

const p = join("docs", "path", "page.jsx");
console.log(p); // docs/path/page.jsx

console.log(basename(p)); // page.jsx
console.log(dirname(p));  // docs/path
console.log(extname(p));  // .jsx`;

export default function PathDoc() {
  return (
    <DocsShell active="/docs/path">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Path handling
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:path</code> module provides utilities for working with file and directory paths. It is similar to Node.js's <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">path</code> module.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Usage</h2>
      <div className="mt-4">
        <CodeBlock code={PATH} title="app.js" lang="js" />
      </div>

      <p className="mt-12 text-sm text-zinc-500">
        For more details on available properties, view the{" "}
        <a href="/api/path" className="font-medium text-brand-600 hover:text-brand-700">
          API Reference
        </a>.
      </p>
    </DocsShell>
  );
}
