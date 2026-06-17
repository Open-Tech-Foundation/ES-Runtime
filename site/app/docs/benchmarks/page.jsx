import DocsShell from "../../../components/DocsShell.jsx";
import BenchChart from "../../../components/BenchChart.jsx";
import BenchStandings from "../../../components/BenchStandings.jsx";
import bench from "../../../src/benchmarks.js";

const startup = [
  { key: "startup", label: "Cold start (near-empty script)", unit: "ms" },
  { key: "bigscript", label: "Parse + run ~100 KB script", unit: "ms" },
  { key: "rss", label: "Peak resident memory", unit: "MB" },
];

const workloads = [
  { key: "crypto", label: "WebCrypto (sign/verify)", unit: "ms" },
  { key: "sha256", label: "SubtleCrypto SHA-256", unit: "ms" },
  { key: "json", label: "JSON parse/stringify", unit: "ms" },
  { key: "jsonbig", label: "JSON (large documents)", unit: "ms" },
  { key: "url", label: "URL parsing", unit: "ms" },
  { key: "url_setter", label: "URL setter", unit: "ms" },
  { key: "urlpattern", label: "URLPattern test", unit: "ms" },
  { key: "encoding", label: "TextEncoder/TextDecoder", unit: "ms" },
  { key: "base64", label: "base64 (atob/btoa)", unit: "ms" },
  { key: "structured", label: "structuredClone", unit: "ms" },
  { key: "compute", label: "Tight compute loop", unit: "ms" },
  { key: "async", label: "async/await throughput", unit: "ms" },
  { key: "timers", label: "setTimeout churn", unit: "ms" },
  { key: "streams", label: "ReadableStream piping", unit: "ms" },
  { key: "fetch", label: "fetch (local server)", unit: "ms" },
  { key: "http", label: "HTTP server (concurrent)", unit: "ms" },
  { key: "fsread_small", label: "File read (small)", unit: "ms" },
  { key: "fsread_large", label: "File read (large)", unit: "ms" },
  { key: "fswrite_small", label: "File write (small)", unit: "ms" },
  { key: "fswrite_large", label: "File write (large)", unit: "ms" },
  { key: "fsappend_small", label: "File append (small)", unit: "ms" },
  { key: "fsappend_large", label: "File append (large)", unit: "ms" },
  { key: "fsstat_small", label: "File stat (small)", unit: "ms" },
  { key: "fsstat_large", label: "File stat (large)", unit: "ms" },
  { key: "fsexists_small", label: "File exists (small)", unit: "ms" },
  { key: "fsexists_large", label: "File exists (large)", unit: "ms" },
  { key: "glob", label: "Glob scan", unit: "ms" },
];

const LABELS = { esrun: "esrun", bun: "Bun", node: "Node.js", deno: "Deno", llrt: "LLRT" };

const versions = Object.keys(bench.runtimes).map((k) => ({
  k: LABELS[k] || k,
  v: bench.runtimes[k],
}));

export default function BenchmarksDoc() {
  return (
    <DocsShell active="/docs/benchmarks">
      <p className="text-sm font-medium text-brand-600">Comparisons</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Benchmarks
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Standard Web APIs, the same script on each runtime (n/a where a runtime
        lacks one). Lower is better. esrun pairs near-instant startup with full
        WinterTC coverage and a JIT engine; LLRT (QuickJS) edges it on cold start
        and memory but has no JIT, server, or fs APIs. Numbers shown as measured.
      </p>

      <div className="mt-6 rounded-xl border border-zinc-200 bg-zinc-50 p-4 text-sm text-zinc-600">
        <div className="font-medium text-zinc-900">Runtimes measured</div>
        <ul className="mt-2 grid gap-1 font-mono text-[12px] sm:grid-cols-2">
          {versions.map((r) => (
            <li>
              <span className="text-brand-700">{r.k}</span>{" "}
              <span className="text-zinc-500">{r.v}</span>
            </li>
          ))}
        </ul>
      </div>

      <div className="mt-4 rounded-xl border border-zinc-200 bg-zinc-50 p-4 text-sm text-zinc-600">
        <div className="font-medium text-zinc-900">How it's measured</div>
        <ul className="mt-2 list-disc space-y-1 pl-5">
          <li>
            Runtimes run <strong>interleaved, in randomized order</strong> each
            repetition — so they share one contention window and the ranking is
            fair, not decided by which ran first.
          </li>
          <li>
            Each cell is the <strong>minimum</strong> over repetitions (the
            contention-free floor), after a discarded warmup.
          </li>
          <li>Cells with high run-to-run variance are flagged in the raw output.</li>
        </ul>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Standings at a glance
      </h2>
      <p className="mt-2 text-sm text-zinc-500">
        Who placed where across every metric — and a quick, neutral take on
        where each runtime fits in production. Pick by fit, not by medal count.
      </p>
      <div className="mt-5">
        <BenchStandings />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Startup &amp; footprint
      </h2>
      <p className="mt-2 text-sm text-zinc-500">
        Process wall-time (min of N runs) and peak resident set on a near-empty
        script.
      </p>
      <div className="mt-5">
        <BenchChart metrics={startup} />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Workloads</h2>
      <p className="mt-2 text-sm text-zinc-500">
        Self-timed with <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">performance.now()</code>{" "}
        after an untimed JIT warmup; min of N interleaved runs, isolating engine
        cost from process launch.
      </p>
      <div className="mt-5">
        <BenchChart metrics={workloads} />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Reproduce</h2>
      <p className="mt-3 text-zinc-600">
        The harness lives in <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">bench/</code>. It
        auto-detects installed runtimes and can emit the JSON that powers these
        charts:
      </p>
      <div className="mt-4 overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950">
        <pre className="overflow-x-auto px-4 py-4 text-[13px] leading-relaxed text-zinc-100">
          <code>{"# human-readable table\nbench/run.sh\n\n# regenerate the data module this page renders\nbench/gen-bench-data.sh"}</code>
        </pre>
      </div>
      <p className="mt-4 text-sm text-zinc-500">
        For CLI/script benchmarking, tools like <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">hyperfine</code>{" "}
        are also a good fit. Numbers vary by hardware; treat them as relative,
        not absolute.
      </p>
    </DocsShell>
  );
}
