const OTF = "https://opentechf.org";
const WEB_FW = "https://web.opentechf.org";

export default function Footer() {
  return (
    <footer className="border-t border-zinc-200 bg-zinc-50">
      <div className="mx-auto flex max-w-6xl flex-col items-center justify-between gap-3 px-6 py-10 text-sm text-zinc-500 sm:flex-row">
        <a href={OTF} target="_blank" rel="noreferrer" className="transition-colors hover:text-brand-600">
          © 2026 Open Tech Foundation
        </a>
        <div className="flex items-center gap-1.5">
          <span>Built with</span>
          <a
            href={WEB_FW}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-zinc-700 transition-colors hover:text-brand-600"
          >
            OTF Web Framework
          </a>
        </div>
      </div>
    </footer>
  );
}
