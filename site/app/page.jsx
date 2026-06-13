import BenchChart from "../components/BenchChart.jsx";
import InstallBox from "../components/InstallBox.jsx";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

const HERO_METRICS = [
  { key: "startup", label: "Cold start", unit: "ms" },
  { key: "rss", label: "Peak memory", unit: "MB" },
  { key: "crypto", label: "WebCrypto", unit: "ms" },
];

const features = [
  {
    title: "Capability-gated",
    body: "Deny-by-default security. Code gets exactly the host powers you grant — no ambient filesystem, network, or environment access.",
  },
  {
    title: "Web standards only",
    body: "Built to the WinterTC Minimum Common Web Platform API — fetch, URL, streams, WebCrypto, encoding, timers, events. No bespoke runtime globals.",
  },
  {
    title: "ESM, and only ESM",
    body: "Standard ES Modules end to end: static imports, dynamic import(), top-level await, import.meta. No CommonJS, no JSON imports, no JSX.",
  },
  {
    title: "Built on V8",
    body: "The engine that powers Chrome and Node, embedded from Rust. A baked startup snapshot boots a realm in milliseconds with a tiny memory footprint.",
  },
  {
    title: "Sandboxed modules",
    body: "node_modules resolution with package exports and pnpm-aware realpath — all confined to a filesystem root jail that is on by default.",
  },
  {
    title: "Embeddable by design",
    body: "A driven event loop with no owned thread. Tick it from your host loop and stay in full control of scheduling and lifetime.",
  },
];

const stats = [
  { value: "7.1ms", label: "cold start to first eval" },
  { value: "18MB", label: "peak resident memory" },
  { value: "0", label: "ambient capabilities by default" },
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
            <p className="text-sm font-semibold uppercase tracking-wider text-brand-600">
              Server-side JavaScript · Web standards
            </p>
            <h1 className="mt-4 text-4xl font-bold leading-[1.05] tracking-tight text-zinc-900 sm:text-5xl lg:text-[3.4rem]">
              A secure, standards-based
              <span className="text-brand-600"> server runtime.</span>
            </h1>
            <p className="mt-6 max-w-xl text-lg leading-relaxed text-zinc-600">
              esrun runs modern ECMAScript on V8 with deny-by-default
              capabilities and a Web-standard API surface — built in Rust to
              embed inside server applications. It is not Node-compatible by
              design.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a
                href="/docs"
                className="rounded-lg bg-brand-600 px-5 py-3 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-brand-700"
              >
                Get started
              </a>
              <a
                href={GITHUB}
                target="_blank"
                rel="noreferrer"
                className="rounded-lg border border-zinc-200 px-5 py-3 text-sm font-semibold text-zinc-700 transition-colors hover:border-zinc-300 hover:bg-zinc-50"
              >
                View on GitHub
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
                  vs Node · Bun · Deno
                </span>
              </div>
              <BenchChart metrics={HERO_METRICS} />
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

      {/* Stats */}
      <section className="border-b border-zinc-200 bg-zinc-50">
        <div className="mx-auto grid max-w-6xl grid-cols-1 divide-y divide-zinc-200 px-6 sm:grid-cols-3 sm:divide-x sm:divide-y-0">
          {stats.map((s) => (
            <div className="px-2 py-8 text-center sm:py-10">
              <div className="text-3xl font-bold tracking-tight text-zinc-900">
                {s.value}
              </div>
              <div className="mt-1 text-sm text-zinc-500">{s.label}</div>
            </div>
          ))}
        </div>
      </section>

      {/* Builtin Core Features */}
      <section className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
        <div className="max-w-2xl">
          <h2 className="text-3xl font-bold tracking-tight text-zinc-900">
            Builtin Core Features
          </h2>
          <p className="mt-3 text-lg text-zinc-600">
            Essential runtime capabilities — every host power is explicit, and
            the engine is confined to a single auditable crate.
          </p>
        </div>

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
                esrun is a runtime, not a toolchain. It deliberately leaves
                package management, building, and testing to other tools — and
                is not a drop-in for Node.
              </p>
              <a
                href="/docs/scope"
                className="mt-5 inline-block text-sm font-semibold text-brand-600 hover:text-brand-700"
              >
                Read the scope &amp; non-goals →
              </a>
            </div>
            <ul className="grid grid-cols-2 gap-x-6 gap-y-2 text-sm text-zinc-600">
              <li>✗ Node.js compatibility</li>
              <li>✗ CommonJS</li>
              <li>✗ TypeScript</li>
              <li>✗ JSX</li>
              <li>✗ JSON imports</li>
              <li>✗ Package installer</li>
              <li>✗ Bundler / linter / formatter</li>
              <li>✗ Test runner</li>
              <li>✗ Watch mode</li>
              <li>✗ FFI</li>
              <li>✗ Workers</li>
              <li>✗ Native addons</li>
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
            Read the docs to embed esrun, or explore the standard{" "}
            <span className="font-mono text-zinc-200">runtime:</span> module
            APIs.
          </p>
          <div className="mt-8 flex flex-wrap items-center justify-center gap-3">
            <a
              href="/docs"
              className="rounded-lg bg-white px-5 py-3 text-sm font-semibold text-zinc-900 transition-colors hover:bg-zinc-100"
            >
              Read the docs
            </a>
            <a
              href="/docs/process"
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
