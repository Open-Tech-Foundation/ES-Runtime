import BenchRoller from "../components/BenchRoller.jsx";
import CodeTabs from "../components/CodeTabs.jsx";
import DurableDiagram from "../components/DurableDiagram.jsx";
import FrameworkTabs from "../components/FrameworkTabs.jsx";
import RuntimeVersions from "../components/RuntimeVersions.jsx";
import SandboxDiagram from "../components/SandboxDiagram.jsx";
import StackIcon from "../components/StackIcon.jsx";
import StatusIcon from "../components/StatusIcon.jsx";

// The two binaries are names, not prose. A tinted chip keeps them readable as
// identifiers without shouting — the site's <code> rule only sets a font, which
// left them looking like ordinary words in a sentence.
const BIN = "rounded-md bg-zinc-200/70 px-1.5 py-0.5 font-mono text-[0.9em] font-semibold text-zinc-900 dark:bg-zinc-800 dark:text-zinc-100";

// The stack, as icon tiles: official artwork where a brand exists, an
// original monochrome glyph where none does (same deal as RuntimeIcon's
// `oj` tile). Groups carry the meaning; labels name the tile. No
// paragraphs — the docs behind each tile already say it once.
const groups = [
  {
    name: "Connect",
    desc: "HTTP and WebSocket servers in the binary.",
    tiles: [
      { icon: "http", label: "HTTP", to: "/docs/http/basics" },
      { icon: "websocket", label: "WebSocket", to: "/docs/guides/websocket" },
    ],
  },
  {
    name: "Store",
    desc: "Embedded SQLite plus first-party Postgres, MySQL/MariaDB, and Redis.",
    tiles: [
      { icon: "sqlite", label: "SQLite", to: "/docs/db" },
      { icon: "postgres", label: "PostgreSQL", to: "/docs/db" },
      { icon: "mysql", label: "MySQL", to: "/docs/db" },
      { icon: "mariadb", label: "MariaDB", to: "/docs/db" },
      { icon: "redis", label: "Redis", to: "/docs/db/redis" },
    ],
  },
  {
    name: "Speak",
    desc: "Parsers for every format your API touches.",
    tiles: [
      { icon: "xml", label: "XML", to: "/docs/serialization/xml" },
      { icon: "yaml", label: "YAML", to: "/docs/serialization/yaml" },
      { icon: "json", label: "JSON", to: "/docs/serialization/jsonl" },
      { icon: "toml", label: "TOML", to: "/docs/serialization/toml" },
      { icon: "messagepack", label: "MessagePack", to: "/docs/serialization/msgpack" },
      { icon: "protobuf", label: "Protobuf", to: "/docs/serialization/protobuf" },
    ],
  },
  {
    name: "Develop",
    desc: "Tests with DOM parity, real-browser runs, watch, HMR, plugins.",
    tiles: [
      { icon: "test", label: "Test runner", to: "/esdev/test" },
      { icon: "dom", label: "DOM", to: "/esdev/test/dom" },
      { icon: "watch", label: "Watcher", to: "/esdev/watch" },
      { icon: "browsers", label: "WebDriver BiDi", to: "/esdev/test/browser" },
      { icon: "plugins", label: "Plugins", to: "/esdev/plugins" },
      { icon: "hmr", label: "HMR", to: "/esdev/start" },
      { icon: "oxc", label: "Bundler", to: "/esdev/build" },
      { icon: "css", label: "CSS", to: "/esdev/build/css" },
      { icon: "tailwind", label: "Tailwind", to: "/esdev/build/tailwind" },
      { icon: "jsx", label: "JSX", to: "/esdev/build/project#jsx" },
    ],
  },
  {
    name: "Run on",
    desc: "A Rust core around V8, scripted in JavaScript and TypeScript.",
    tiles: [
      { icon: "wasm", label: "WebAssembly", to: "/docs/wasm" },
      { icon: "v8", label: "V8", to: "/docs/comparison" },
      { icon: "rust", label: "Rust", to: "/docs/comparison" },
      { icon: "javascript", label: "JavaScript", to: "/docs" },
      { icon: "typescript", label: "TypeScript", to: "/esdev/typescript" },
    ],
  },
  {
    name: "Built-in Utils",
    desc: "SMTP mail, password-grade hashing, two-factor codes, cookies, CSRF tokens, and webhooks.",
    tiles: [
      { icon: "email", label: "Email", to: "/docs/guides/email" },
      { icon: "hashing", label: "Hashing", to: "/docs/guides/hashing" },
      { icon: "cookies", label: "Cookies", to: "/docs/http/basics#cookies" },
      { icon: "csrf", label: "CSRF", to: "/docs/http/basics#csrf" },
      { icon: "webhooks", label: "Webhooks", to: "/docs/guides/webhooks" },
      { icon: "totp", label: "2FA", to: "/docs/guides/hashing#two-factor-sign-in" },
    ],
  },
  {
    name: "Ships with",
    desc: "Native image processing and durable state.",
    tiles: [
      { icon: "images", label: "Images", to: "/docs/guides/images" },
      { icon: "durable", label: "Durable workers", to: "/docs/guides/durable-workers" },
    ],
  },
  {
    name: "Secure",
    desc: "Deny by default on a capability-based runtime.",
    tiles: [
      { icon: "sandbox", label: "Deny by default", to: "/docs/guides/securing-runtime" },
      { icon: "capabilities", label: "Capabilities", to: "/docs/guides/securing-runtime/capabilities" },
    ],
  },
  {
    name: "Observe",
    desc: "OTLP spans to any collector, no SDK.",
    tiles: [{ icon: "otel", label: "OTel export", to: "/docs/guides/diagnostics" }],
  },
];


// Durable workers: each point is a property the test suite or the shop
// benchmark proves, not a number — the measurements live on the internals page,
// generated by the committed scripts, and are linked rather than restated.
const durablePoints = [
  {
    title: "One call at a time, per worker",
    body: "Calls to one worker queue and run in order, so two customers cannot both take the last item. No lock to take, no row to contend for.",
  },
  {
    title: "Answered once it is on disk",
    body: "A result is released only after the writes behind it have committed. A crash is a call that never returned, never one that returned a lie.",
  },
  {
    title: "Timers that outlive the process",
    body: "An alarm is stored beside the state and runs after a restart: retries, abandoned carts, a nightly roll-up, without a scheduler service.",
  },
  {
    title: "Nothing else to run",
    body: "State lives in embedded SQLite, one file per worker, under the same read and write grants as everything else. No Redis, no queue, no database server.",
  },
];

// What the *production* binary deliberately lacks — each one available in esdev,
// on your machine. These used to sit in the red list below, which read as "you
// cannot", when the truth is "not in the binary you deploy".
const devItems = [
  "TypeScript & JSX",
  "CommonJS (converted by esdev)",
  "Bundler",
  "Test runner",
  "Watch mode",
  "Debugger",
  "Permission discovery",
];

// Non-goals for the whole project, not a division of labour between binaries.
const scopeItems = [
  "Node.js compatibility",
  "CommonJS at runtime",
  "Package installer",
  "Linter / formatter",
  "FFI",
  "Native addons",
  "Remote module imports",
  "SharedWorker / data: worker URLs",
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
              V8-based ECMAScript runtime for the server. WinterTC-compliant,
              capability secured.
            </p>
            <div className="mt-8 flex flex-wrap items-center gap-3">
              <a
                href="/docs"
                className="inline-flex items-center gap-2 rounded-full border border-zinc-200 bg-white px-5 py-3 text-sm font-semibold text-zinc-900 shadow-sm transition-colors hover:border-zinc-300 hover:bg-zinc-50 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-100 dark:hover:border-zinc-600 dark:hover:bg-zinc-800"
              >
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" className="text-brand-500"><path d="M4.5 16.5c-1.5 1.26-2 5-2 5s3.74-.5 5-2c.71-.84.7-2.13-.09-2.91a2.18 2.18 0 0 0-2.91-.09z"/><path d="m12 15-3-3a22 22 0 0 1 2-3.95A12.88 12.88 0 0 1 22 2c0 2.72-.78 7.5-3.05 11a22.35 22.35 0 0 1-3.95 2z"/><path d="M9 12H4s.55-3.03 2-4c1.62-1.08 5 0 5 0"/><path d="M12 15v5s3.03-.55 4-2c1.08-1.62 0-5 0-5"/></svg>
                Get started
              </a>
            </div>
            <div className="mt-6 max-w-xl">
              <CodeTabs />
            </div>
          </div>

          {/* Benchmark card hugs the right edge; the text column hugs the left. */}
          <div className="w-full lg:max-w-md lg:justify-self-end">
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
              <div className="mb-5 flex items-baseline justify-between">
                <h2 className="text-sm font-semibold text-zinc-900 dark:text-zinc-100">
                  Benchmarks
                </h2>
              </div>
              <BenchRoller />
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

          {/* Two explicit columns, not CSS masonry: only placement puts
              Develop top-right on purpose. Cards keep their own heights. */}
          <div className="mx-auto grid max-w-5xl gap-6 sm:grid-cols-2">
            {[
              ["Connect", "Store", "Speak", "Ships with", "Secure"],
              ["Develop", "Run on", "Built-in Utils", "Observe"],
            ].map((column, ci) => (
              <div className="space-y-6">
                {column.map((name) => {
                  const gi = groups.findIndex((g) => g.name === name);
                  const g = groups[gi];
                  return (
                    <div
                      className="why-card group relative overflow-hidden rounded-2xl border border-zinc-200 bg-white p-6 transition duration-200 hover:-translate-y-0.5 hover:border-brand-200 hover:shadow-md dark:border-zinc-800 dark:bg-zinc-900 dark:hover:border-brand-900"
                      style={{ animationDelay: `${gi * 70}ms` }}
                    >
                      {/* Faint brand wash, only on hover. */}
                      <div className="pointer-events-none absolute -right-10 -top-10 h-28 w-28 rounded-full bg-brand-500/0 blur-2xl transition-colors duration-300 group-hover:bg-brand-500/15" />
                      <h3 className="relative text-xs font-bold uppercase tracking-widest text-zinc-500 dark:text-zinc-400">
                        {g.name}
                      </h3>
                <p className="relative mt-1.5 border-l-2 border-brand-500 pl-3 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">
                  {g.desc}
                </p>
                      <div className="relative mt-4 flex flex-wrap gap-x-6 gap-y-5">
                        {g.tiles.map((t) => (
                          <a
                            href={t.to}
                            className="tile-link flex w-20 flex-col items-center gap-2 rounded-lg text-center transition duration-200 hover:-translate-y-0.5 focus-visible:outline-2 focus-visible:outline-brand-500"
                          >
                            <StackIcon name={t.icon} />
                            <span className="text-[13px] font-medium leading-tight text-zinc-700 dark:text-zinc-300">
                              {t.label}
                            </span>
                          </a>
                        ))}
                      </div>
                    </div>
                  );
                })}
              </div>
            ))}
          </div>

          <p className="mx-auto mt-10 max-w-3xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Choosing a runtime?{" "}
            <a
              href="/docs/comparison"
              className="font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
            >
              How esrun compares to Node.js, Bun and Deno →
            </a>
          </p>
        </div>
      </section>

      {/* Durable workers: the one thing here no other runtime ships in the
          box, shown as the story the shop example tells rather than as code.
          Properties, not numbers — the numbers are on the internals page,
          where the scripts that produce them are named. */}
      <section id="durable-workers" className="border-b border-zinc-200 dark:border-zinc-800">
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="mb-12 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
              State without infrastructure
            </h2>
            <p className="mt-4 text-lg text-zinc-600 dark:text-zinc-400">
              Durable workers: addressable state that outlives the process.
            </p>
          </div>
          <div className="mx-auto max-w-5xl rounded-2xl bg-zinc-900 p-6 shadow-xl ring-1 ring-white/10 sm:p-8 dark:shadow-none">
            <DurableDiagram />
          </div>
          <div className="mx-auto mt-12 grid max-w-5xl gap-8 sm:grid-cols-2 lg:grid-cols-4">
            {durablePoints.map((p) => (
              <div>
                <h3 className="text-base font-semibold text-zinc-900 dark:text-zinc-100">{p.title}</h3>
                <p className="mt-2 text-sm leading-relaxed text-zinc-600 dark:text-zinc-400">{p.body}</p>
              </div>
            ))}
          </div>
          <p className="mx-auto mt-10 max-w-3xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Killed with <code className="font-mono">SIGKILL</code> under load, the example shop loses no
            order it acknowledged.
          </p>
          <div className="mt-4 flex flex-wrap justify-center gap-x-6 gap-y-2">
            <a
              href="/docs/internals/durable-workers#on-real-storage"
              className="text-sm font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
            >
              What it costs on real storage →
            </a>
            <a
              href="/api/workers"
              className="text-sm font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
            >
              Durable workers API →
            </a>
          </div>
        </div>
      </section>

      {/* Frameworks and dev servers: throughput per runtime, startup per
          tool. A full viewport, like a slide — the comparison is the whole
          point, so it gets the whole screen rather than sharing one. The
          numbers come from the generated benchmark data, never typed by
          hand. */}
      <section className="flex min-h-screen flex-col justify-center border-b border-zinc-200 dark:border-zinc-800">
        <div className="mx-auto w-full max-w-6xl px-6 py-20">
          <div className="mb-8 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
              Benchmarks
            </h2>
          </div>
          <FrameworkTabs />
        </div>
      </section>

      {/* Sandbox Section. The section follows the site theme like every other;
          only the diagram itself is dark in both, because the brand orange and
          the six host colours need a dark ground to carry — and a single dark
          slab on a light page is what marks it as the one thing here that is
          ours rather than table stakes. */}
      <section>
        <div className="mx-auto max-w-6xl px-6 py-20 lg:py-24">
          <div className="mb-12 text-center">
            <h2 className="text-3xl font-bold tracking-tight text-zinc-900 dark:text-zinc-100">
              Deny by default
            </h2>
            <p className="mt-4 text-lg text-zinc-600 dark:text-zinc-400">
              A run reaches what the command line named, and nothing else.
            </p>
          </div>
          <div className="mx-auto max-w-5xl rounded-2xl bg-zinc-900 p-6 shadow-xl ring-1 ring-white/10 sm:p-8 dark:shadow-none">
            <SandboxDiagram />
          </div>
          <p className="mx-auto mt-6 max-w-5xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Eleven capabilities: <code className="font-mono">read</code>, <code className="font-mono">write</code>,{" "}
            <code className="font-mono">imports</code>, <code className="font-mono">net</code>,{" "}
            <code className="font-mono">listen</code>, <code className="font-mono">env</code>,{" "}
            <code className="font-mono">run</code>, <code className="font-mono">signals</code>,{" "}
            <code className="font-mono">workers</code>, <code className="font-mono">diagnostics</code>,{" "}
            <code className="font-mono">diagnostics-detail</code>. The gate is the op, not the import.
          </p>
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
                <code className={BIN}>esrun</code>{" "}
                runs your service and nothing else — no watcher, no debugger port,
                no transform. That narrowness is what a capability flag is worth.
              </p>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                <code className={BIN}>esdev</code>{" "}
                pays for it on your machine: same runtime, same flags, plus the
                toolchain to get there.
              </p>
              <div className="mt-6 flex flex-wrap gap-x-6 gap-y-2">
                <a
                  href="/esdev"
                  className="text-sm font-semibold text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
                >
                  <code className="font-mono font-semibold">esdev</code> docs →
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
                  In <code className="font-mono normal-case text-zinc-700 dark:text-zinc-300">esdev</code>, not in production
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
              <h3 className="text-xl font-bold text-zinc-900 dark:text-zinc-100">Start building</h3>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                Install both binaries with one command, and run your first module. Standards-based, fast, ESM-only.
              </p>
              <div className="mt-6">
                <a
                  href="/docs"
                  className="inline-flex items-center rounded-lg bg-zinc-900 px-5 py-3 text-sm font-semibold text-white transition-colors hover:bg-zinc-800 dark:bg-zinc-100 dark:text-zinc-900 dark:hover:bg-zinc-200"
                >
                  Read the docs
                </a>
              </div>
            </div>
            <div className="rounded-2xl border border-zinc-200 bg-white p-8 dark:border-zinc-800 dark:bg-zinc-900">
              <h3 className="text-xl font-bold text-zinc-900 dark:text-zinc-100">Ship it locked down</h3>
              <p className="mt-3 text-zinc-600 dark:text-zinc-400">
                Nothing is granted unless the command line says so. Build the deploy line that states exactly what your service may touch.
              </p>
              <div className="mt-6">
                <a
                  href="/docs/guides/securing-runtime"
                  className="inline-flex items-center rounded-lg border border-zinc-300 bg-transparent px-5 py-3 text-sm font-semibold text-zinc-900 transition-colors hover:bg-zinc-100 dark:border-zinc-700 dark:text-zinc-100 dark:hover:bg-zinc-800"
                >
                  Securing a deployment
                </a>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
