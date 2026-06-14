// Shared docs chrome: a left sidebar + a centered prose column. Each docs page
// wraps its body in <DocsShell active="/docs/...">. Self-contained so it does
// not depend on nested-layout behaviour in the router.

const NAV = [
  {
    group: "Getting started",
    items: [
      { href: "/docs", label: "Overview" },
      { href: "/docs/scope", label: "Scope & non-goals" },
    ],
  },
  {
    group: "Comparisons",
    items: [
      { href: "/docs/comparison", label: "vs Node.js · Bun · Deno" },
      { href: "/docs/benchmarks", label: "Benchmarks" },
    ],
  },
  {
    group: "Web standard APIs",
    items: [
      { href: "/docs/globals", label: "Global objects" },
    ],
  },
  {
    group: "Runtime",
    items: [
      { href: "/docs/modules", label: "Module system" },
      { href: "/docs/security", label: "Security model" },
    ],
  },
];

export default function DocsShell({ active, children }) {
  return (
    <div className="mx-auto flex max-w-6xl gap-10 px-6 py-12">
      <aside className="hidden w-56 shrink-0 lg:block">
        <nav className="sticky top-24 space-y-7">
          {NAV.map((section) => (
            <div>
              <div className="mb-2 text-xs font-semibold uppercase tracking-wider text-zinc-400">
                {section.group}
              </div>
              <ul className="space-y-0.5">
                {section.items.map((item) => (
                  <li>
                    <a
                      href={item.href}
                      className={
                        "block rounded-md px-3 py-1.5 text-sm transition-colors " +
                        (item.href === active
                          ? "bg-brand-50 font-medium text-brand-700"
                          : "text-zinc-600 hover:bg-zinc-50 hover:text-zinc-900")
                      }
                    >
                      {item.label}
                    </a>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </nav>
      </aside>

      <article className="min-w-0 flex-1 pb-8">{children}</article>
    </div>
  );
}
