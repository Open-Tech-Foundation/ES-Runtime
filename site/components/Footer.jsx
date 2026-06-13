const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

export default function Footer() {
  return (
    <footer className="border-t border-zinc-200 bg-zinc-50">
      <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-4 px-6 py-10 sm:flex-row">
        <div className="flex items-center gap-2.5">
          <span className="flex h-6 w-6 items-center justify-center rounded-md bg-indigo-600 font-mono text-xs font-bold text-white">
            e
          </span>
          <span className="text-sm text-zinc-500">
            esrun — a secure, embeddable JavaScript runtime.
          </span>
        </div>
        <div className="flex items-center gap-6 text-sm text-zinc-500">
          <a href="/docs" className="transition-colors hover:text-zinc-900">
            Docs
          </a>
          <a href="/docs/process" className="transition-colors hover:text-zinc-900">
            API Reference
          </a>
          <a
            href={GITHUB}
            target="_blank"
            rel="noreferrer"
            className="transition-colors hover:text-zinc-900"
          >
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
