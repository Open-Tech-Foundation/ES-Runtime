import pkg from "../package.json";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

export default function Footer() {
  return (
    <footer className="border-t border-zinc-200 bg-zinc-50">
      <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-4 px-6 py-10 sm:flex-row">
        <div className="text-sm text-zinc-500">
          <span className="font-mono font-semibold text-zinc-700">esrun</span>
          {" "}
          v{pkg.version} — a secure, standards-based server-side JavaScript
          runtime.
        </div>
        <div className="flex items-center gap-6 text-sm text-zinc-500">
          <a href="/docs" className="transition-colors hover:text-brand-600">
            Docs
          </a>
          <a href="/docs/process" className="transition-colors hover:text-brand-600">
            API Reference
          </a>
          <a
            href={GITHUB}
            target="_blank"
            rel="noreferrer"
            className="transition-colors hover:text-brand-600"
          >
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
