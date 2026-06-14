import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const IMPORT = `import { join, resolve, dirname, fromFileURL } from "runtime:path";

// The modern __dirname: the directory of the current module.
const here = dirname(fromFileURL(import.meta.url));
const cfg = resolve(here, "config", "app.json");`;

const exports = [
  { sig: "sep", type: "string", desc: 'Path segment separator for the host OS ("/" or "\\").' },
  { sig: "delimiter", type: "string", desc: 'Path list delimiter for the host OS (":" or ";").' },
  { sig: "isAbsolute(p)", type: "(string) => boolean", desc: "Whether p is an absolute path." },
  { sig: "normalize(p)", type: "(string) => string", desc: "Collapses . / .. and redundant separators." },
  { sig: "join(...segments)", type: "(...string) => string", desc: "Joins segments with the separator, then normalizes." },
  { sig: "resolve(...segments)", type: "(...string) => string", desc: "Resolves to an absolute path, anchoring at cwd() if no segment is absolute." },
  { sig: "dirname(p)", type: "(string) => string", desc: "The directory portion of p." },
  { sig: "basename(p)", type: "(string) => string", desc: "The final segment of p (no suffix-stripping overload)." },
  { sig: "extname(p)", type: "(string) => string", desc: "The extension of the final segment, including the dot (or empty)." },
  { sig: "parse(p)", type: "(string) => object", desc: "{ root, dir, base, name, ext }." },
  { sig: "relative(from, to)", type: "(string, string) => string", desc: "Relative path from from to to (both resolved first)." },
  { sig: "fromFileURL(url)", type: "(string | URL) => string", desc: "Converts a file: URL to a path." },
  { sig: "toFileURL(p)", type: "(string) => URL", desc: "Converts a path (resolved to absolute) to a file: URL." },
];

export default function PathDoc() {
  return (
    <ApiShell active="/api/path">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:path
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Modern, platform-aware path utilities. Pure computation — no I/O. The
        host platform and working directory come from runtime:process, so
        separators and resolve() follow the real OS.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: Env
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · runtime: scheme
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
      </div>

      <p className="mt-8 text-zinc-600">
        One platform-correct surface — no <code className="font-mono">posix</code>/
        <code className="font-mono">win32</code> dual namespaces and no overloaded
        signatures — plus first-class <code className="font-mono">file:</code> URL
        interop:{" "}
        <code className="font-mono">dirname(fromFileURL(import.meta.url))</code> is
        the modern <code className="font-mono">__dirname</code>.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Import</h2>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="runtime:path" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Exports</h2>
      <div className="mt-5 space-y-4">
        {exports.map((e) => (
          <div className="rounded-xl border border-zinc-200 p-5">
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
              <code className="font-mono text-[15px] font-semibold text-zinc-900">
                {e.sig}
              </code>
              <code className="font-mono text-[13px] text-zinc-400">
                {e.type}
              </code>
            </div>
            <p className="mt-2 text-sm leading-relaxed text-zinc-600">
              {e.desc}
            </p>
          </div>
        ))}
      </div>
    </ApiShell>
  );
}
