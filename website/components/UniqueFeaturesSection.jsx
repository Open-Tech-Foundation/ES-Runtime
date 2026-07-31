const items = [
  {
    title: "One core, two shapes",
    body: "The esrun binary and the embeddable Rust library ship from the same engine.",
    icon: (
      <>
        <rect x="3" y="3" width="7" height="7" rx="1.5" />
        <rect x="14" y="14" width="7" height="7" rx="1.5" />
        <path d="M10 6.5h4.5a2 2 0 0 1 2 2V14" />
      </>
    ),
  },
  {
    title: "I/O you inject",
    body: "Filesystem, network, clock, and env arrive as provider traits. Nothing is ambient.",
    icon: (
      <>
        <path d="M12 3v6" />
        <path d="m9 6 3-3 3 3" />
        <rect x="3" y="11" width="18" height="10" rx="2" />
        <path d="M7 16h.01M11 16h6" />
      </>
    ),
  },
  {
    title: "A loop you drive",
    body: "No owned thread. Tick the runtime from your host loop and keep scheduling and lifetime.",
    icon: (
      <>
        <path d="M21 12a9 9 0 1 1-3.5-7.1" />
        <path d="M21 3v5h-5" />
        <circle cx="12" cy="12" r="2.5" />
      </>
    ),
  },
  {
    title: "Capabilities are granted",
    body: "Embedded code starts with zero powers. The host hands out exactly what it needs.",
    icon: (
      <>
        <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
        <path d="m9 12 2 2 4-4" />
      </>
    ),
  },
  {
    title: "Snapshot boot",
    body: "A baked V8 snapshot opens a full WinterTC realm in 7 ms, at 20 MB peak resident memory.",
    href: "/docs/benchmarks",
    icon: (
      <>
        <path d="M13 2 4.5 13.5H11l-1 8.5 9.5-11.5H13z" />
      </>
    ),
  },
  {
    title: "One binary",
    body: "Self-contained, checksum-verified, no asset directory and no runtime dependencies.",
    icon: (
      <>
        <path d="m12 2 8.5 4.8v10.4L12 22l-8.5-4.8V6.8z" />
        <path d="M12 22V12" />
        <path d="m3.5 6.8 8.5 5 8.5-5" />
      </>
    ),
  },
];

export default function UniqueFeaturesSection() {
  return (
    <section className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
      <div className="mb-12 text-center">
        <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
          Distinctly ESRun
        </h2>
      </div>

      <div className="relative overflow-hidden rounded-2xl border border-zinc-800 bg-zinc-950 p-8 shadow-inner lg:p-10">
        {/* Subtle background glow */}
        <div className="pointer-events-none absolute left-1/2 top-0 h-32 w-3/4 -translate-x-1/2 bg-brand-500/10 blur-[100px]"></div>

        <div className="relative z-10 grid grid-cols-1 gap-6 md:grid-cols-2 lg:grid-cols-3">
          {items.map((item) => (
            <div className="group flex flex-col rounded-xl border-t-4 border-zinc-800 border-t-brand-500 bg-zinc-900/80 p-5 shadow-lg transition-colors hover:bg-zinc-900">
              <svg
                width="24"
                height="24"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="text-brand-400"
                aria-hidden="true"
              >
                {item.icon}
              </svg>
              <h3 className="mt-4 font-bold tracking-wide text-zinc-100">
                {item.title}
              </h3>
              <p className="mt-2 text-[13px] leading-relaxed text-zinc-400">
                {item.body}
              </p>
              {item.href ? (
                <a
                  href={item.href}
                  className="mt-3 text-xs font-medium text-brand-400 hover:text-brand-300"
                >
                  See benchmarks →
                </a>
              ) : null}
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
