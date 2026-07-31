const items = [
  {
    title: "Parsers already inside",
    body: "XML, YAML, TOML, JSONL, MessagePack, and Protobuf ship in the binary. No install step.",
    href: "/docs/serialization/xml",
    linkText: "Serialization docs →",
    icon: (
      <>
        <path d="M9 3H5a2 2 0 0 0-2 2v4" />
        <path d="M15 3h4a2 2 0 0 1 2 2v4" />
        <path d="M9 21H5a2 2 0 0 1-2-2v-4" />
        <path d="M15 21h4a2 2 0 0 0 2-2v-4" />
        <path d="M8 12h8" />
      </>
    ),
  },
  {
    title: "A server, not a framework",
    body: "HTTP and WebSockets are built in and speak plain Request and Response.",
    href: "/docs/http",
    linkText: "HTTP docs →",
    icon: (
      <>
        <rect x="2" y="3" width="20" height="7" rx="2" />
        <rect x="2" y="14" width="20" height="7" rx="2" />
        <path d="M6 6.5h.01M6 17.5h.01" />
      </>
    ),
  },
  {
    title: "Subprocesses that stream",
    body: "Pipe a request body straight into ffmpeg or git, and the output straight back out.",
    href: "/docs/guides/subprocess",
    linkText: "Subprocess docs →",
    icon: (
      <>
        <rect x="2.5" y="4" width="19" height="16" rx="2" />
        <path d="m7 9 3 3-3 3" />
        <path d="M13 15h4" />
      </>
    ),
  },
  {
    title: "Web standards, no dialect",
    body: "fetch, URL, URLPattern, streams, WebCrypto. Host powers arrive as runtime: modules, not new globals.",
    href: "/docs/globals",
    linkText: "Global objects →",
    icon: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M3 12h18" />
        <path d="M12 3a15 15 0 0 1 0 18a15 15 0 0 1 0-18z" />
      </>
    ),
  },
  {
    title: "Starts in 7 ms",
    body: "A baked V8 snapshot opens a full WinterTC realm at 20 MB peak resident memory.",
    href: "/docs/benchmarks",
    linkText: "See benchmarks →",
    icon: (
      <>
        <path d="M13 2 4.5 13.5H11l-1 8.5 9.5-11.5H13z" />
      </>
    ),
  },
  {
    title: "One binary to ship",
    body: "Self-contained and checksum-verified. No asset directory, no runtime dependencies.",
    href: "/docs/install",
    linkText: "Install →",
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
            <div className="flex flex-col rounded-xl border-t-4 border-zinc-800 border-t-brand-500 bg-zinc-900/80 p-5 shadow-lg transition-colors hover:bg-zinc-900">
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
              <a
                href={item.href}
                className="mt-auto pt-3 text-xs font-medium text-brand-400 hover:text-brand-300"
              >
                {item.linkText}
              </a>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
