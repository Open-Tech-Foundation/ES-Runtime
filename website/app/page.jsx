import BenchRoller from "../components/BenchRoller.jsx";
import CodeTabs from "../components/CodeTabs.jsx";
import RpsChart from "../components/RpsChart.jsx";
import RuntimeVersions from "../components/RuntimeVersions.jsx";
import StatusIcon from "../components/StatusIcon.jsx";
import bench from "../src/benchmarks.js";

// Read from the generated data rather than restating it. This line said "about
// 7 ms" while the suite measured 7.9, which is how every hand-typed number on a
// site starts: correct once.
const startupMs = bench.results_ms?.startup?.esrun;
const startupPhrase = typeof startupMs === "number" ? `about ${Math.round(startupMs)} ms` : "milliseconds";

const features = [
  {
    title: "Standard web APIs",
    body: "fetch, URL, URLPattern, streams, WebCrypto, encoding, timers, and events — the WinterTC Minimum Common Web Platform API. What you learn here is what runs in the browser.",
  },
  {
    title: "Batteries in the binary",
    body: "HTTP and WebSocket servers, filesystem, sockets, subprocesses, WASM/WASI, and XML, YAML, TOML, JSONL, MessagePack, and Protobuf parsers — all behind runtime: imports, nothing to install.",
  },
  {
    title: "ESM everywhere",
    body: "Static imports, dynamic import(), top-level await, import.meta, and JSON modules. One module system, no CommonJS interop rules to learn — esdev's bundler converts a CJS dependency at build time.",
  },
  {
    title: "Written in Rust",
    body: "The runtime around the engine is safe Rust: no data races, no use-after-free, no buffer overruns to reach for under hostile input.",
  },
  {
    title: "Powered by V8",
    body: `The engine behind Chrome and Node.js, embedded from Rust. A baked startup snapshot opens a full realm in ${startupPhrase}.`,
  },
  {
    title: "Embeddable & capability-gated",
    body: "The library form is deny-by-default — the host grants exactly the powers code needs — and its event loop has no owned thread: tick it from your own.",
  },
];

// What the *production* binary deliberately lacks — each one available in esdev,
// on your machine. These used to sit in the red list below, which read as "you
// cannot", when the truth is "not in the binary you deploy".
const devItems = [
  "TypeScript & JSX",
  "CommonJS (converted at build)",
  "Bundler",
  "Test runner",
  "Watch mode",
  "Debugger",
];

// Non-goals for the whole project, not a division of labour between binaries.
const scopeItems = [
  "Node.js compatibility",
  "Package installer",
  "Linter / formatter",
  "FFI",
  "Native addons",
  "Remote module imports",
];

export default function HomePage() {
  return (
    <div>
      {/* Hero */}
      <section className="relative overflow-hidden border-b border-zinc-200 dark:border-zinc-800">
        <div className="hero-dot-grid pointer-events-none absolute inset-0 opacity-[0.4] dark:opacity-[0.25]" />
        <div className="relative mx-auto grid max-w-6xl gap-12 px-6 py-20 lg:grid-cols-2 lg:items-center lg:py-24">
          <div>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-brand-50 px-3 py-1 text-xs font-semibold uppercase tracking-wider text-brand-700 ring-1 ring-inset ring-brand-200 dark:bg-brand-950/50 dark:text-brand-300 dark:ring-brand-800">
              <span className="h-1.5 w-1.5 rounded-full bg-brand-500" />
              Alpha
            </span>
            <h1 className="mt-4 text-4xl font-bold leading-[1.05] tracking-tight text-zinc-900 dark:text-zinc-50 sm:text-5xl lg:text-[3.4rem]">
              A secure, standards-based
              <span className="text-brand-600 dark:text-brand-400"> server runtime.</span>
            </h1>
            <p className="mt-6 max-w-xl text-lg leading-relaxed text-zinc-600 dark:text-zinc-400">
              V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable,
              capability-secured.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a
                href="/docs"
                className="inline-flex items-center gap-2 rounded-full border border-zinc-200 bg-white px-5 py-3 text-sm font-semibold text-zinc-900 shadow-sm transition-colors hover:border-zinc-300 hover:bg-zinc-50 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100 dark:hover:border-zinc-600 dark:hover:bg-zinc-800"
              >
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-brand-500"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-3.05 11a22.35 22.35 0 0 1-3.95 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/></svg>
                Get started
              </a>
            </div>
            <div className="mt-6 max-w-xl">
              <CodeTabs />
            </div>
          </div>

          {/* Benchmark chart replaces the usage snippet. */}
          <div className="lg:pl-4">
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
              <div className="mb-5 flex items-baseline justify-between">
                <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                  Benchmarks
                </h2>
              </div>
              <RpsChart />
              <div className="mt-5 border-t border-zinc-100 pt-5 dark:border-zinc-800">
                <BenchRoller />
              </div>
              <a
                href="/docs/benchmarks"
                className="mt-5 inline-block text-xs font-medium text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
              >
                See full benchmarks →
              </a>
            </div>
          </div>
        </div>
      </section>

      {/* Why ES-Runtime? */}
      <section className="border-b border-zinc-200 bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-950">
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="mb-12 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
              Why ES-Runtime?
            </h2>
          </div>

          <div className="grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
            {features.map((f) => (
              <div className="group relative overflow-hidden rounded-2xl border border-zinc-200 bg-white p-6 transition duration-200 hover:-translate-y-0.5 hover:border-brand-200 hover:shadow-md dark:border-zinc-800 dark:bg-zinc-900 dark:hover:border-brand-900">
                {/* Faint brand wash, only on hover. */}
                <div className="pointer-events-none absolute -right-10 -top-10 h-28 w-28 rounded-full bg-brand-500/0 blur-2xl transition-colors duration-300 group-hover:bg-brand-500/15" />
                <h3 className="relative text-base font-semibold text-zinc-900 dark:text-zinc-100">
                  {f.title}
                </h3>
                <span className="mt-3 block h-[3px] w-8 rounded-full bg-brand-500/70 transition-all duration-200 group-hover:w-12 group-hover:bg-brand-500" />
                <p className="relative mt-3 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
                  {f.body}
                </p>
              </div>
            ))}
          </div>
        </div>
      </section>

      {/* Architecture Section */}
      <section>
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="mb-12 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
              Simple Runtime Architecture
            </h2>
            <p className="mt-4 text-lg text-zinc-600 dark:text-zinc-400">
              How your JavaScript code interacts with the system.
            </p>
          </div>
          <div className="mx-auto flex max-w-4xl flex-col items-center justify-between gap-8 rounded-2xl border border-zinc-200 bg-white p-8 shadow-sm dark:border-zinc-800 dark:bg-zinc-900 md:flex-row">
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-white p-2 shadow-sm ring-1 ring-zinc-200 dark:bg-zinc-800 dark:ring-zinc-700">
                <img
                  src="/img/v8.svg"
                  alt="V8"
                  width="40"
                  height="40"
                  className="h-10 w-10 dark:hidden"
                />
                <img
                  src="/img/v8-outline.svg"
                  alt="V8"
                  width="40"
                  height="40"
                  className="hidden h-10 w-10 dark:block"
                />
              </div>
              <h3 className="mt-4 font-semibold text-zinc-900 dark:text-zinc-100">JavaScript</h3>
              <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-400">Executes your JS/ESM code at lightning speed.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-orange-100 dark:bg-orange-950/50">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-orange-600 dark:text-orange-400"><polygon points="12 2 2 7 12 12 22 7 12 2"/><polyline points="2 17 12 22 22 17"/><polyline points="2 12 12 17 22 12"/></svg>
              </div>
              <h3 className="mt-4 font-semibold text-zinc-900 dark:text-zinc-100">Op Layer</h3>
              <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-400">Drives the event loop and low-cost boundary calls.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-blue-100 dark:bg-blue-950/50">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-blue-600 dark:text-blue-400"><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/></svg>
              </div>
              <h3 className="mt-4 font-semibold text-zinc-900 dark:text-zinc-100">Runtime Modules</h3>
              <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-400">Standard Web APIs like fetch, crypto, and streams.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-zinc-100 text-2xl dark:bg-zinc-800">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-zinc-600 dark:text-zinc-400"><rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect><rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect><line x1="6" y1="6" x2="6.01" y2="6"></line><line x1="6" y1="18" x2="6.01" y2="18"></line></svg>
              </div>
              <h3 className="mt-4 font-semibold text-zinc-900 dark:text-zinc-100">OS Level</h3>
              <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-400">Kernel networking, file system, and raw system I/O.</p>
            </div>
          </div>
        </div>
      </section>

      {/* Scope / non-goals teaser */}
      <section className="border-t border-zinc-200 bg-zinc-50 dark:border-zinc-800 dark:bg-zinc-950">
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="grid gap-12 lg:grid-cols-2 lg:items-center lg:gap-16">
            <div>
              <h2 className="text-2xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
                Two binaries.
              </h2>
              <p className="mt-4 text-zinc-600 dark:text-zinc-400">
                <code className="font-mono text-sm text-zinc-900 dark:text-zinc-100">esrun</code>{" "}
                runs your service and nothing else — no watcher, no debugger port,
                no transform. That narrowness is what a capability flag is worth.
              </p>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                <code className="font-mono text-sm text-zinc-900 dark:text-zinc-100">esdev</code>{" "}
                pays for it on your machine: same runtime, same flags, plus the
                toolchain to get there.
              </p>
              <div className="mt-6 flex flex-wrap gap-x-6 gap-y-2">
                <a
                  href="/docs/esdev"
                  className="text-sm font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
                >
                  esdev docs →
                </a>
                <a
                  href="/docs/scope"
                  className="text-sm font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
                >
                  Scope &amp; non-goals →
                </a>
              </div>
            </div>
            <div className="grid gap-8 sm:grid-cols-2">
              <div>
                <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-500">
                  In esdev, not in production
                </h3>
                <ul className="mt-4 space-y-3.5 text-sm leading-snug text-zinc-600 dark:text-zinc-400">
                  {devItems.map((label) => (
                    <li className="flex min-h-7 items-center gap-3 py-1">
                      <span className="flex size-[18px] shrink-0 items-center justify-center">
                        <StatusIcon status="yes" className="block size-[18px] text-emerald-500" />
                      </span>
                      <span>{label}</span>
                    </li>
                  ))}
                </ul>
              </div>
              <div>
                <h3 className="text-xs font-semibold uppercase tracking-wider text-zinc-500 dark:text-zinc-500">
                  Not goals at all
                </h3>
                <ul className="mt-4 space-y-3.5 text-sm leading-snug text-zinc-600 dark:text-zinc-400">
                  {scopeItems.map((label) => (
                    <li className="flex min-h-7 items-center gap-3 py-1">
                      <span className="flex size-[18px] shrink-0 items-center justify-center">
                        <StatusIcon status="no" className="block size-[18px] text-rose-500" />
                      </span>
                      <span>{label}</span>
                    </li>
                  ))}
                </ul>
              </div>
            </div>
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="border-t border-zinc-200 dark:border-zinc-800">
        <div className="mx-auto max-w-6xl px-6 py-16 lg:py-20">
          <h2 className="text-center text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
            Ship a runtime you can trust.
          </h2>
          <div className="mt-12 grid gap-10 sm:grid-cols-2 text-left">
            <div className="rounded-2xl border border-zinc-200 bg-white p-8 dark:border-zinc-800 dark:bg-zinc-900">
              <h3 className="text-xl font-bold text-zinc-900 dark:text-zinc-100">Standard Server Runtime</h3>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                A standard-based, full-capability, fast and optimal runtime for general workloads.
              </p>
              <div className="mt-6">
                <a
                  href="/docs"
                  className="inline-flex items-center rounded-lg bg-zinc-900 px-5 py-3 text-sm font-semibold text-white transition-colors hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                >
                  Read ESRun Docs
                </a>
              </div>
            </div>
            <div className="rounded-2xl border border-zinc-200 bg-white p-8 dark:border-zinc-800 dark:bg-zinc-900">
              <h3 className="text-xl font-bold text-zinc-900 dark:text-zinc-100">Embeddable Engine</h3>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                Embedded for ultimate control & untrusted code execution. Inject standard APIs or create your own custom capabilities from Rust.
              </p>
              <div className="mt-6">
                <a
                  href="/docs/embed"
                  className="inline-flex items-center rounded-lg border border-zinc-300 bg-transparent px-5 py-3 text-sm font-semibold text-zinc-900 transition-colors hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-100 dark:hover:bg-zinc-800"
                >
                  Embeddable Guide
                </a>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
