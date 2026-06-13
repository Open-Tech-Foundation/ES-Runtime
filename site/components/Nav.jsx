import pkg from "../package.json";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

export default function Nav() {
  return (
    <header className="sticky top-0 z-40 border-b border-zinc-200/80 bg-white/80 backdrop-blur">
      <nav className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
        <a href="/" className="flex items-baseline gap-2">
          <span className="font-mono text-[17px] font-bold tracking-tight text-zinc-900">
            esrun
          </span>
          <span className="rounded-full border border-zinc-200 px-2 py-0.5 text-[11px] font-medium tabular-nums text-zinc-500">
            v{pkg.version}
          </span>
        </a>

        <div className="flex items-center gap-1 sm:gap-2">
          <a
            href="/docs"
            className="rounded-md px-3 py-2 text-sm font-medium text-zinc-600 transition-colors hover:text-zinc-900"
          >
            Docs
          </a>
          <a
            href="/docs/process"
            className="hidden rounded-md px-3 py-2 text-sm font-medium text-zinc-600 transition-colors hover:text-zinc-900 sm:inline-block"
          >
            API
          </a>
          <a
            href={GITHUB}
            target="_blank"
            rel="noreferrer"
            className="ml-1 rounded-md border border-zinc-200 px-3 py-2 text-sm font-medium text-zinc-700 transition-colors hover:border-brand-300 hover:bg-brand-50 hover:text-brand-700"
          >
            GitHub
          </a>
        </div>
      </nav>
    </header>
  );
}
