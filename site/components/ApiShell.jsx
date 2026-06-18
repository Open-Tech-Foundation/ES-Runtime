// Shared chrome for the API reference: a left sidebar + a centered prose
// column, mirroring DocsShell. Each API page wraps its body in
// <ApiShell active="/api/...">.

const NAV = [
  {
    group: "Reference",
    items: [
      { href: "/api", label: "Overview" },
      { href: "/api/cli", label: "CLI" },
      { href: "/api/process", label: "runtime:process" },
      { href: "/api/path", label: "runtime:path" },
      { href: "/api/fs", label: "runtime:fs" },
      { href: "/api/net", label: "runtime:net" },
      { href: "/api/http", label: "runtime:http" },
      { href: "/api/websocket", label: "runtime:websocket" },
    ],
  },
];

export default function ApiShell({ active, children }) {
  const allItems = NAV.flatMap(section => section.items);
  const currentIndex = allItems.findIndex(item => item.href === active);
  const nextItem = currentIndex !== -1 && currentIndex < allItems.length - 1 ? allItems[currentIndex + 1] : null;

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

      <article className="min-w-0 flex-1 pb-8">
        {children}
        {nextItem && (
          <div className="mt-16 flex justify-end border-t border-zinc-200 pt-8">
            <a
              href={nextItem.href}
              className="group flex items-center text-sm font-medium text-zinc-900 hover:text-brand-600"
            >
              {nextItem.label}
              <span className="ml-2 block transition-transform group-hover:translate-x-1">
                →
              </span>
            </a>
          </div>
        )}
      </article>
    </div>
  );
}
