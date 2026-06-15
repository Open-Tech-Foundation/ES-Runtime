import BenchRoller from "../components/BenchRoller.jsx";
import RpsChart from "../components/RpsChart.jsx";
import InstallBox from "../components/InstallBox.jsx";
import StatusIcon from "../components/StatusIcon.jsx";

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
    body: "Static imports, dynamic import(), top-level await, import.meta. No CommonJS, no JSON imports, no JSX.",
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
                className="inline-flex items-center gap-2 rounded-lg border border-zinc-200 bg-white px-5 py-3 text-sm font-semibold text-zinc-900 shadow-sm transition-colors hover:border-zinc-300 hover:bg-zinc-50"
              >
                <span>🚀</span> Get started
              </a>
            </div>
            <div className="mt-6 max-w-xl">
              <InstallBox />
            </div>
          </div>

          {/* Benchmark chart replaces the usage snippet. */}
          <div className="lg:pl-4">
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm">
              <div className="mb-5 flex items-baseline justify-between">
                <h2 className="text-sm font-semibold text-zinc-900">
                  Benchmarks
                </h2>
                <span className="text-xs text-zinc-400">
                  vs Node.js · Bun · Deno
                </span>
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
              <li className="flex items-center gap-2"><StatusIcon status="no" className="inline-block h-[18px] w-[18px] shrink-0 text-rose-500" /> JSON imports</li>
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
        <div className="mx-auto max-w-6xl px-6 py-16 text-center lg:py-20">
          <h2 className="text-3xl font-bold tracking-tight text-white">
            Ship a runtime you can trust.
          </h2>
          <p className="mx-auto mt-4 max-w-xl text-lg text-zinc-400">
            Run untrusted JavaScript with only the capabilities you grant —
            embed the library, or use the <span className="font-mono text-zinc-200">esrun</span> CLI.
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
            <a
              href="/docs"
              className="rounded-lg bg-white px-5 py-3 text-sm font-semibold text-zinc-900 transition-colors hover:bg-zinc-100"
            >
              Read the docs
            </a>
            <a
              href="/api"
              className="rounded-lg border border-zinc-700 px-5 py-3 text-sm font-semibold text-zinc-200 transition-colors hover:bg-zinc-900"
            >
              API reference
            </a>
          </div>
        </div>
      </section>
    </div>
  );
}
