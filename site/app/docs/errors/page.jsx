import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

export default function ErrorDiagnostics() {
  return (
    <DocsShell active="/docs/errors">
      <p className="text-sm font-medium text-brand-600">Runtime</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Error Diagnostics
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime guarantees native exception class preservation and accurate stack
        trace reporting. Instead of swallowing errors as generic string blocks, the
        runtime retains exact subclasses (like <code className="rounded bg-zinc-100 px-1 py-0.5 text-[13px]">TypeError</code>, <code className="rounded bg-zinc-100 px-1 py-0.5 text-[13px]">DOMException</code>) and precise source mapping points.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Stack Traces</h2>
      <p className="mt-3 leading-relaxed text-zinc-600">
        When an unhandled exception or unhandled promise rejection bubbles to the 
        top level, <code className="rounded bg-zinc-100 px-1 py-0.5 text-[13px]">esrun</code> 
        elegantly formats the error trace in your CLI:
      </p>

      <div className="mt-4">
        <CodeBlock code={`error: uncaught exception in my-script.mjs\nTypeError: network connection refused\n    at fetchData (file:///path/to/script.mjs:10:5)\n    at file:///path/to/script.mjs:2:1`} title="Terminal" lang="text" />
      </div>

      <div className="mt-6 rounded-xl border border-brand-200 bg-brand-50 p-5 leading-relaxed text-brand-900">
        <strong>Good to know:</strong> The CLI handles color formatting seamlessly and adjusts gracefully 
        if piped into other tools or if <code className="rounded bg-white/70 px-1.5 py-0.5 text-[13px]">NO_COLOR</code> is set.
      </div>
    </DocsShell>
  );
}
