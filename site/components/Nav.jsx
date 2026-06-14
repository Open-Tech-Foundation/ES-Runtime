import { router } from "@opentf/web/router";
import pkg from "../package.json";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

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
    <header className="sticky top-0 z-40 border-b border-zinc-200/80 bg-white/80 backdrop-blur">
      <nav className="mx-auto flex h-16 max-w-6xl items-center justify-between px-6">
        <a href="/" className="flex items-baseline gap-2">
          <span className="text-[17px] font-bold tracking-tight text-zinc-900">
            ES <span className="text-brand-600">Runtime</span>
          </span>
          <span className="rounded-full border border-zinc-200 px-2 py-0.5 text-[11px] font-medium tabular-nums text-zinc-500">
            v{pkg.version}
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
            className="ml-1 rounded-md border border-zinc-200 px-3 py-2 text-sm font-medium text-zinc-700 transition-colors hover:border-brand-300 hover:bg-brand-50 hover:text-brand-700"
          >
            GitHub
          </a>
        </div>
      </nav>
    </header>
  );
}
