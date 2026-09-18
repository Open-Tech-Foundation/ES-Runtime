// Tab switcher for the homepage Benchmarks section: request throughput
// (Hono vs Elysia per runtime), dev-server startup (vite vs oj vs esdev vs
// bun) and production build time (same four tools). Every panel reads the
// generated benchmark data; the tab state is the only thing this component
// owns.
import BuildChart from "./BuildChart.jsx";
import DevServerChart from "./DevServerChart.jsx";
import RpsChart from "./RpsChart.jsx";

function tabClass(active) {
  return active
    ? "rounded-full px-4 py-1.5 text-sm font-semibold text-zinc-900 bg-white shadow-xs dark:bg-zinc-700 dark:text-zinc-100 transition-all"
    : "rounded-full px-4 py-1.5 text-sm font-medium text-zinc-500 hover:text-zinc-800 dark:text-zinc-400 dark:hover:text-zinc-200 transition-colors";
}

export default function FrameworkTabs() {
  let tab = $state("req");

  return (
    <div>
      <div className="mb-8 flex justify-center">
        <div className="flex items-center gap-1 rounded-full bg-zinc-100 p-1 dark:bg-zinc-800/80">
          <button type="button" onclick={() => (tab = "req")} className={tabClass(tab === "req")}>
            Request throughput
          </button>
          <button type="button" onclick={() => (tab = "dev")} className={tabClass(tab === "dev")}>
            Dev-server startup
          </button>
          <button type="button" onclick={() => (tab = "build")} className={tabClass(tab === "build")}>
            Build time
          </button>
        </div>
      </div>

      {tab === "req" ? (
        <div>
          <div className="grid gap-6 lg:grid-cols-2">
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
              <RpsChart server="hono" title="Hono hello-world · Speed & Memory" />
            </div>
            <div className="rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
              <RpsChart server="elysia" title="Elysia hello-world · Speed & Memory" />
            </div>
          </div>
          <p className="mx-auto mt-8 max-w-3xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Plaintext hello-world over loopback, driven by oha at 100
            connections — best of three runs. Elysia is measured on the esdev
            bundle every runtime serves (esrun is ESM-only), and Node serves
            both frameworks through the same{" "}
            <code className="font-mono">@hono/node-server</code> glue, so the
            delta is route handling, not the adapter.{" "}
            <a
              href="/docs/benchmarks"
              className="font-medium text-brand-600 hover:text-brand-700 dark:text-brand-400 dark:hover:text-brand-300"
            >
              How it is measured →
            </a>
          </p>
        </div>
      ) : tab === "dev" ? (
        <div>
          <div className="mx-auto max-w-4xl rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
            <DevServerChart />
          </div>
          <p className="mx-auto mt-8 max-w-3xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Same 10,000-component React app booted under{" "}
            <code className="font-mono">vite dev</code>,{" "}
            <code className="font-mono">oj dev --bundle</code>,{" "}
            <code className="font-mono">esdev start</code> and{" "}
            <code className="font-mono">bun ./index.html</code> — spawn to
            first paint in a real browser, min of three cold+warm sessions;
            memory is peak RSS. The fixture shape follows oj's published
            bench.
          </p>
        </div>
      ) : (
        <div>
          <div className="mx-auto max-w-4xl rounded-2xl border border-zinc-200 bg-white p-6 shadow-sm dark:border-zinc-800 dark:bg-zinc-900">
            <BuildChart />
          </div>
          <p className="mx-auto mt-8 max-w-3xl text-center text-sm text-zinc-500 dark:text-zinc-400">
            Same app, minified production build, min of three runs — every
            leg minifies and builds production React (bun needs an explicit{" "}
            <code className="font-mono">NODE_ENV=production</code>, which the
            other three default to). Each output must mount in a real browser
            before its numbers publish.
          </p>
        </div>
      )}
    </div>
  );
}
