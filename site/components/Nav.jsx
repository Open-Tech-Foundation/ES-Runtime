import { router } from "@opentf/web/router";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

// Injected from the workspace Cargo.toml at build time (see vite.config.js) —
// the released runtime version, not this site's package.json.
const VERSION = __RUNTIME_VERSION__;

const LINKS = [
  { href: "/", label: "Home" },
  { href: "/docs", label: "Docs" },
  { href: "/api", label: "API" },
];

// Active when the path matches exactly, or (for non-root links) is nested under
// it. `router.pathname` is reactive, so reading it inside className keeps the
// underline in sync as you navigate.
function isActive(href) {
  const path = router.pathname;
  return href === "/" ? path === "/" : path === href || path.startsWith(href + "/");
}

export default function Nav() {
  return (
    <header className="fixed inset-x-0 top-0 z-40 border-b border-zinc-200/80 bg-white/80 backdrop-blur">
      <nav className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
        <a href="/" className="flex items-baseline gap-2">
          <span className="text-[17px] font-bold tracking-tight text-zinc-900">
            ES <span className="text-brand-600">Runtime</span>
          </span>
          <span className="rounded-full border border-zinc-200 px-2 py-0.5 text-[11px] font-medium tabular-nums text-zinc-500">
            v{VERSION}
          </span>
        </a>

        <div className="flex items-center gap-1 sm:gap-2">
          {LINKS.map((l) => (
            <a
              href={l.href}
              className={
                "relative rounded-md px-3 py-2 text-sm font-medium transition-colors " +
                (isActive(l.href)
                  ? "text-zinc-900 after:absolute after:inset-x-3 after:-bottom-px after:h-0.5 after:rounded-full after:bg-brand-500"
                  : "text-zinc-600 hover:text-zinc-900")
              }
            >
              {l.label}
            </a>
          ))}
          <a
            href={GITHUB}
            target="_blank"
            rel="noreferrer"
            aria-label="GitHub repository"
            title="GitHub"
            className="ml-1 rounded-md p-2 text-zinc-500 transition-colors hover:bg-zinc-100 hover:text-zinc-900"
          >
            <svg viewBox="0 0 24 24" fill="currentColor" className="h-5 w-5" aria-hidden="true">
              <path d="M12 .5C5.37.5 0 5.87 0 12.5c0 5.3 3.44 9.8 8.21 11.39.6.11.82-.26.82-.58 0-.29-.01-1.05-.02-2.06-3.34.73-4.04-1.61-4.04-1.61-.55-1.39-1.33-1.76-1.33-1.76-1.09-.74.08-.73.08-.73 1.2.09 1.84 1.24 1.84 1.24 1.07 1.83 2.81 1.3 3.49.99.11-.78.42-1.3.76-1.6-2.67-.3-5.47-1.34-5.47-5.96 0-1.31.47-2.39 1.24-3.23-.13-.31-.54-1.53.12-3.18 0 0 1.01-.32 3.3 1.23a11.5 11.5 0 0 1 6.01 0c2.29-1.55 3.3-1.23 3.3-1.23.66 1.65.25 2.87.12 3.18.77.84 1.23 1.92 1.23 3.23 0 4.63-2.81 5.65-5.49 5.95.43.37.81 1.1.81 2.22 0 1.6-.01 2.89-.01 3.28 0 .32.21.7.82.58A12 12 0 0 0 24 12.5C24 5.87 18.63.5 12 .5z" />
            </svg>
          </a>
        </div>
      </nav>
    </header>
  );
}
