import BenchRoller from "../components/BenchRoller.jsx";
import RpsChart from "../components/RpsChart.jsx";
import StatusIcon from "../components/StatusIcon.jsx";
import WhyChooseSection from "../components/WhyChooseSection.jsx";

const features = [
  {
    title: "Capability-gated",
    body: "The embeddable library is deny-by-default: the host grants exactly the powers code needs — no ambient filesystem, network, or environment access.",
  },
  {
    title: "Web standards only",
    body: "The WinterTC Minimum Common Web Platform API: fetch, URL, streams, WebCrypto, encoding, timers, events. No bespoke globals.",
  },
  {
    title: "ESM, and only ESM",
    body: "Static imports, dynamic import(), top-level await, import.meta, and JSON modules. No CommonJS, no JSX.",
  },
  {
    title: "Built on Rust",
    body: "Memory-safe by construction — no data races, no use-after-free. The host stays crash-resistant even under hostile input.",
  },
  {
    title: "Built on V8",
    body: "The engine behind Chrome and Node.js, embedded from Rust. A baked snapshot boots a realm in milliseconds.",
  },
  {
    title: "Embeddable by design",
    body: "A driven event loop with no owned thread. Tick it from your host loop and keep full control of scheduling and lifetime.",
  },
];

export default function HomePage() {
  return (
    <div>
      {/* Hero */}
      <section className="relative overflow-hidden border-b border-zinc-200">
        <div
          className="pointer-events-none absolute inset-0 opacity-[0.4]"
          style="background-image: radial-gradient(circle at 1px 1px, #e4e4e7 1px, transparent 0); background-size: 28px 28px;"
        />
        <div className="relative mx-auto grid max-w-6xl gap-12 px-6 py-20 lg:grid-cols-2 lg:items-center lg:py-24">
          <div>
            <span className="inline-flex items-center gap-1.5 rounded-full bg-brand-50 px-3 py-1 text-xs font-semibold uppercase tracking-wider text-brand-700 ring-1 ring-inset ring-brand-200">
              <span className="h-1.5 w-1.5 rounded-full bg-brand-500" />
              Alpha
            </span>
            <h1 className="mt-4 text-4xl font-bold leading-[1.05] tracking-tight text-zinc-900 sm:text-5xl lg:text-[3.4rem]">
              A secure, standards-based
              <span className="text-brand-600"> server runtime.</span>
            </h1>
            <p className="mt-6 max-w-xl text-lg leading-relaxed text-zinc-600">
              V8-based ECMAScript runtime, WinterTC-compliant, I/O-injectable,
              capability-secured.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a
                href="/docs"
                className="inline-flex items-center gap-2 rounded-full border border-zinc-200 bg-white px-5 py-3 text-sm font-semibold text-zinc-900 shadow-sm transition-colors hover:border-zinc-300 hover:bg-zinc-50"
              >
                <span>🚀</span> Get started
              </a>
            </div>
            <div className="mt-6 max-w-xl">
              <h3 className="mb-3 text-sm font-semibold text-zinc-900">Simple HTTP Server</h3>
              <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950 px-4 py-3.5">
                <pre className="text-[13px] leading-relaxed text-zinc-300 overflow-x-auto whitespace-pre">
                  <code>
                    <span className="text-brand-400">import</span> {"{ serve }"} <span className="text-brand-400">from</span> <span className="text-emerald-300">"runtime:http"</span>;{"\n\n"}
                    <span className="text-blue-300">serve</span>{"({ "}port: <span className="text-orange-300">8080</span>{" }, "}
                    <span className="text-brand-400">async</span>{" (req) "}
                    <span className="text-brand-400">=&gt;</span>{" {\n"}
                    {"  "}
                    <span className="text-brand-400">return new</span>{" Response("}
                    <span className="text-emerald-300">"👋 Hello from ESRun!"</span>{");\n"}
                    {"});\n\n"}
                    <span className="text-blue-300">console</span>.log(<span className="text-emerald-300">"Server listening on port 8080"</span>);{"\n"}
                  </code>
                </pre>
              </div>
            </div>
          </div>

          {/* Benchmark chart replaces the usage snippet. */}
          <div className="lg:pl-4">
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm">
              <div className="mb-5 flex items-baseline justify-between">
                <h2 className="text-sm font-semibold text-zinc-900">
                  Benchmarks
                </h2>
              </div>
              <RpsChart />
              <div className="mt-5 border-t border-zinc-100 pt-5">
                <BenchRoller />
              </div>
              <a
                href="/docs/benchmarks"
                className="mt-5 inline-block text-xs font-medium text-brand-600 hover:text-brand-700"
              >
                See full benchmarks →
              </a>
            </div>
          </div>
        </div>
      </section>

      {/* Architecture Section */}
      <section className="border-t border-zinc-200 bg-zinc-50">
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="mb-12 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900">
              Simple Runtime Architecture
            </h2>
            <p className="mt-4 text-lg text-zinc-600">
              How your JavaScript code interacts with the system.
            </p>
          </div>
          <div className="mx-auto flex max-w-4xl flex-col items-center justify-between gap-8 rounded-2xl border border-zinc-200 bg-white p-8 shadow-sm md:flex-row">
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-brand-100 text-xl font-black text-brand-700">V8</div>
              <h3 className="mt-4 font-semibold text-zinc-900">JavaScript</h3>
              <p className="mt-2 text-sm text-zinc-600">Executes your JS/ESM code at lightning speed.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-orange-100 text-2xl">🦀</div>
              <h3 className="mt-4 font-semibold text-zinc-900">Op Layer</h3>
              <p className="mt-2 text-sm text-zinc-600">Drives the event loop and low-cost boundary calls.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-blue-100 text-2xl">🧩</div>
              <h3 className="mt-4 font-semibold text-zinc-900">Runtime Modules</h3>
              <p className="mt-2 text-sm text-zinc-600">Standard Web APIs like fetch, crypto, and streams.</p>
            </div>
            <div className="hidden text-brand-400 md:block">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="M5 12h14"/><path d="m12 5 7 7-7 7"/></svg>
            </div>
            <div className="flex-1 text-center">
              <div className="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-zinc-100 text-2xl">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-zinc-600"><rect x="2" y="2" width="20" height="8" rx="2" ry="2"></rect><rect x="2" y="14" width="20" height="8" rx="2" ry="2"></rect><line x1="6" y1="6" x2="6.01" y2="6"></line><line x1="6" y1="18" x2="6.01" y2="18"></line></svg>
              </div>
              <h3 className="mt-4 font-semibold text-zinc-900">OS Level</h3>
              <p className="mt-2 text-sm text-zinc-600">Kernel networking, file system, and raw system I/O.</p>
            </div>
          </div>
        </div>
      </section>

      {/* Why Choose Section */}
      <WhyChooseSection />

      {/* Builtin Core Features */}
      <section className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
        <h2 className="text-3xl font-bold tracking-tight text-zinc-900">
          Builtin Core Features
        </h2>

        <div className="mt-12 grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
          {features.map((f) => (
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 transition-shadow hover:shadow-sm">
              <h3 className="text-base font-semibold text-zinc-900">
                {f.title}
              </h3>
              <p className="mt-2 text-sm leading-relaxed text-zinc-600">
                {f.body}
              </p>
            </div>
          ))}
        </div>
      </section>

      {/* Scope / non-goals teaser */}
      <section className="border-t border-zinc-200 bg-zinc-50">
        <div className="mx-auto max-w-6xl px-6 py-16 lg:py-20">
          <div className="grid gap-10 lg:grid-cols-2">
            <div>
              <h2 className="text-2xl font-bold tracking-tight text-zinc-900">
                A focused scope.
              </h2>
              <p className="mt-3 text-zinc-600">
                A runtime, not a toolchain — and not a Node.js drop-in. Package
                management, building, and testing are left to other tools.
              </p>
              <a
                href="/docs/scope"
                className="mt-5 inline-block text-sm font-semibold text-brand-600 hover:text-brand-700"
              >
                Read the scope &amp; non-goals →
              </a>
            </div>
            <ul className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm text-zinc-600">
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Node.js compatibility</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> CommonJS</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> TypeScript</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> JSX</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Package installer</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Bundler / linter / formatter</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Test runner</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Watch mode</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> FFI</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Workers</li>
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> Native addons</li>
            </ul>
          </div>
        </div>
      </section>

      {/* CTA */}
      <section className="border-t border-zinc-200 bg-zinc-950">
        <div className="mx-auto max-w-6xl px-6 py-16 lg:py-20">
          <h2 className="text-center text-3xl font-bold tracking-tight text-white">
            Ship a runtime you can trust.
          </h2>
          <div className="mt-12 grid gap-10 sm:grid-cols-2 text-left">
            <div className="rounded-2xl border border-zinc-800 bg-zinc-900/50 p-8">
              <h3 className="text-xl font-bold text-white">Standard Server Runtime</h3>
              <p className="mt-3 text-zinc-400">
                A standard-based, full-capability, fast and optimal runtime for general workloads.
              </p>
              <div className="mt-6">
                <a
                  href="/docs"
                  className="inline-flex items-center rounded-lg bg-white px-5 py-3 text-sm font-semibold text-zinc-900 transition-colors hover:bg-zinc-100"
                >
                  Read ESRun Docs
                </a>
              </div>
            </div>
            <div className="rounded-2xl border border-zinc-800 bg-zinc-900/50 p-8">
              <h3 className="text-xl font-bold text-white">Embeddable Engine</h3>
              <p className="mt-3 text-zinc-400">
                Embedded for ultimate control & untrusted code execution. Inject standard APIs or create your own custom capabilities from Rust.
              </p>
              <div className="mt-6">
                <a
                  href="/docs/embed"
                  className="inline-flex items-center rounded-lg border border-zinc-700 bg-transparent px-5 py-3 text-sm font-semibold text-zinc-200 transition-colors hover:bg-zinc-800"
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
