import CodeBlock from "../components/CodeBlock.jsx";

const GITHUB = "https://github.com/Open-Tech-Foundation/ES-Runtime";

const HERO_CODE = `// hello.mjs
import { env, args } from "runtime:process";

const name = args[0] ?? env.USER ?? "world";
console.log(\`hello, \${name}\`);`;

const features = [
  {
    title: "Capability-gated",
    body: "Deny-by-default security. Code gets exactly the host powers you grant — no ambient filesystem, network, or environment access.",
  },
  {
    title: "ESM-only",
    body: "Standard ES Modules end to end. Static imports, dynamic import(), top-level await, and import.meta — no CommonJS, ever.",
  },
  {
    title: "Built on V8",
    body: "The same engine that powers Chrome and Node, embedded from Rust. A baked startup snapshot boots a realm in milliseconds.",
  },
  {
    title: "WinterTC-aligned",
    body: "Implements the Minimum Common Web Platform API, so the code you write targets a standard server-side surface, not a bespoke one.",
  },
  {
    title: "Sandboxed modules",
    body: "node_modules resolution with package exports, subpath patterns, and pnpm-aware realpath — all confined to a filesystem root jail.",
  },
  {
    title: "Embeddable by design",
    body: "A driven event loop with no owned thread. Tick it from your host loop and stay in full control of scheduling and lifetime.",
  },
];

const stats = [
  { value: "~6.6ms", label: "cold start to first eval" },
  { value: "0", label: "ambient capabilities by default" },
  { value: "ESM", label: "the only module system" },
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
        <div className="relative mx-auto grid max-w-6xl gap-12 px-6 py-20 lg:grid-cols-2 lg:items-center lg:py-28">
          <div>
            <div className="inline-flex items-center gap-2 rounded-full border border-zinc-200 bg-white px-3 py-1 text-xs font-medium text-zinc-600">
              <span className="h-1.5 w-1.5 rounded-full bg-indigo-500" />
              WinterTC-aligned · V8 · Rust
            </div>
            <h1 className="mt-6 text-4xl font-bold leading-[1.05] tracking-tight text-zinc-900 sm:text-5xl lg:text-6xl">
              A secure, embeddable
              <span className="text-indigo-600"> JavaScript runtime.</span>
            </h1>
            <p className="mt-6 max-w-xl text-lg leading-relaxed text-zinc-600">
              esrun runs modern ECMAScript on V8 with deny-by-default
              capabilities and a sandboxed module system — built in Rust to
              embed inside your own application.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a
                href="/docs"
                className="rounded-lg bg-indigo-600 px-5 py-3 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-indigo-500"
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
          </div>

          <div className="lg:pl-4">
            <CodeBlock code={HERO_CODE} title="hello.mjs" lang="js" />
            <div className="mt-3 flex items-center gap-2 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 font-mono text-[13px] text-zinc-700">
              <span className="text-zinc-400">$</span>
              <span>esrun hello.mjs Ada</span>
              <span className="ml-auto text-zinc-400">hello, Ada</span>
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

      {/* Features */}
      <section className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
        <div className="max-w-2xl">
          <h2 className="text-3xl font-bold tracking-tight text-zinc-900">
            Built for hosts that take security seriously.
          </h2>
          <p className="mt-4 text-lg text-zinc-600">
            Every capability is explicit. The engine is confined to one crate,
            and the host decides what the script may touch.
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
