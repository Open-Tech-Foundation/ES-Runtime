const TAB_BASE = "-mb-px border-b-2 px-4 py-2 text-[13px] font-semibold transition-colors ";

const EXAMPLES = [
  {
    id: "http",
    title: "HTTP Server",
    code: (
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
    )
  },
  {
    id: "fs",
    title: "File System",
    code: (
      <code>
        <span className="text-brand-400">import</span> {"{ file }"} <span className="text-brand-400">from</span> <span className="text-emerald-300">"runtime:fs"</span>;{"\n\n"}
        <span className="text-brand-400">const</span> config = <span className="text-blue-300">file</span>(<span className="text-emerald-300">"./config.json"</span>);{"\n\n"}
        <span className="text-blue-300">console</span>.log(<span className="text-brand-400">await</span> config.<span className="text-blue-300">json</span>());{"\n"}
      </code>
    )
  },
  {
    id: "glob",
    title: "Glob Scanning",
    code: (
      <code>
        <span className="text-brand-400">import</span> {"{ Glob }"} <span className="text-brand-400">from</span> <span className="text-emerald-300">"runtime:fs"</span>;{"\n\n"}
        <span className="text-brand-400">for await</span> (<span className="text-brand-400">const</span> match <span className="text-brand-400">of new</span> <span className="text-blue-300">Glob</span>(<span className="text-emerald-300">"**/*.ts"</span>).<span className="text-blue-300">scan</span>()) {"{\n"}
        {"  "}<span className="text-blue-300">console</span>.log(match); <span className="text-zinc-500">// prints matched TypeScript file paths</span>{"\n"}
        {"}\n"}
      </code>
    )
  }
];

export default function CodeTabs() {
  let activeId = $state("http");

  return (
    <div className="overflow-hidden rounded-xl border border-zinc-800 bg-zinc-950">
      <div className="flex items-center gap-1 border-b border-zinc-800 px-2 pt-2">
        {EXAMPLES.map((ex) => (
          <button
            type="button"
            onclick={() => (activeId = ex.id)}
            className={
              TAB_BASE +
              (activeId === ex.id
                ? "border-brand-500 text-white"
                : "border-transparent text-zinc-400 hover:text-zinc-200")
            }
          >
            {ex.title}
          </button>
        ))}
      </div>
      <div className="px-4 py-3.5">
        <pre className="text-[13px] leading-relaxed text-zinc-300 overflow-x-auto whitespace-pre">
          {EXAMPLES.find((ex) => ex.id === activeId).code}
        </pre>
      </div>
    </div>
  );
}
