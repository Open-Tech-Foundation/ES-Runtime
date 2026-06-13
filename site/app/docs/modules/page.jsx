import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const STATIC = `// Relative and bare specifiers both work.
import { greet } from "./greet.mjs";
import greeter from "greeter";           // from node_modules (ESM)

export const message = greet("world");`;

const DYNAMIC = `// Dynamic import() is fully supported, including top-level await.
const { default: plugin } = await import("./plugins/auth.mjs");
await plugin.init();`;

const BUILTIN = `// Host functionality is imported under the runtime: scheme.
import { env, args } from "runtime:process";

console.log(env.HOME, args);`;

export default function ModulesDoc() {
  return (
    <DocsShell active="/docs/modules">
      <p className="text-sm font-medium text-brand-600">Concepts</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Module system
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun loads standard ES Modules. Static imports, dynamic{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">import()</code>,
        top-level await, and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">import.meta.url</code>{" "}
        all behave as specified.
      </p>

      <div className="mt-6 rounded-xl border border-zinc-200 bg-zinc-50 p-5 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Not supported, by design:</strong>{" "}
        CommonJS (<code className="rounded bg-white px-1.5 py-0.5 text-[12px]">require</code> /{" "}
        <code className="rounded bg-white px-1.5 py-0.5 text-[12px]">module.exports</code>), JSON module
        imports (<code className="rounded bg-white px-1.5 py-0.5 text-[12px]">import data from "./x.json"</code>),
        import attributes, JSX, and TypeScript. esrun runs JavaScript ES
        Modules — transpile anything else ahead of time. See{" "}
        <a href="/docs/scope" className="font-medium text-brand-600 hover:text-brand-700">
          Scope &amp; non-goals
        </a>
        .
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Static imports</h2>
      <div className="mt-4">
        <CodeBlock code={STATIC} title="app.mjs" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Dynamic import</h2>
      <div className="mt-4">
        <CodeBlock code={DYNAMIC} title="app.mjs" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Resolving packages
      </h2>
      <p className="mt-3 text-zinc-600">
        Bare specifiers resolve from <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">node_modules</code>,
        honoring the package <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">exports</code> map —
        including conditional <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">import</code>/<code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">default</code> conditions and
        subpath patterns such as <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">"./*"</code>. Symlinks
        are resolved to their real path, so pnpm's nested store works as-is. A
        CommonJS-only package is rejected with a clear error.
      </p>
      <div className="mt-4 rounded-xl border border-amber-200 bg-amber-50 p-4 text-sm text-amber-900">
        <strong>Filesystem root jail.</strong> Module resolution is confined to a
        project root. A specifier that escapes the root is refused, even via
        symlink — the sandbox is the default, not an option.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        The runtime: scheme
      </h2>
      <p className="mt-3 text-zinc-600">
        Built-in host APIs are not globals. They are imported under the{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:</code> scheme, which makes every
        host dependency explicit and statically visible in the source.
      </p>
      <div className="mt-4">
        <CodeBlock code={BUILTIN} title="app.mjs" lang="js" />
      </div>
      <p className="mt-4 text-zinc-600">
        Each built-in module is backed by host ops that carry the capability
        check — the security boundary is the op, not the JavaScript. The first
        shipped module is{" "}
        <a href="/docs/process" className="font-medium text-brand-600 hover:text-brand-500">
          runtime:process
        </a>
        ; <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:fs</code>,{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:net</code>, and{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:http</code> are on the roadmap.
      </p>
    </DocsShell>
  );
}
