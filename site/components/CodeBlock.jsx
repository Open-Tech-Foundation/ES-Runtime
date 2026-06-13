// A minimal dark code panel. Pass `code` as a string and an optional `title`.
// No syntax highlighting by design — crisp, dependency-free, enterprise-clean.

export default function CodeBlock({ code, title, lang }) {
  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950 shadow-sm">
      {title && (
        <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-2.5">
          <span className="text-xs font-medium text-zinc-400">{title}</span>
          {lang && (
            <span className="text-[10px] font-semibold uppercase tracking-wider text-zinc-600">
              {lang}
            </span>
          )}
        </div>
      )}
      <pre className="overflow-x-auto px-4 py-4 text-[13px] leading-relaxed text-zinc-100">
        <code>{code}</code>
      </pre>
    </div>
  );
}
