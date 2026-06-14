import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const IMPORT = `import { join, resolve, dirname, fromFileURL } from "runtime:path";

// The modern __dirname: the directory of the current module.
const here = dirname(fromFileURL(import.meta.url));
const cfg = resolve(here, "config", "app.json");`;

const exports = [
  { sig: "sep", type: "string", desc: 'Path segment separator for the host OS ("/" or "\\").', ex: `sep; // "/" on POSIX, "\\\\" on Windows` },
  { sig: "delimiter", type: "string", desc: 'Path list delimiter for the host OS (":" or ";").', ex: `env.PATH.split(delimiter);` },
  { sig: "isAbsolute(p)", type: "(string) => boolean", desc: "Whether p is an absolute path.", ex: `isAbsolute("/etc/hosts"); // true` },
  { sig: "normalize(p)", type: "(string) => string", desc: "Collapses . / .. and redundant separators.", ex: `normalize("/a/./b/../c"); // "/a/c"` },
  { sig: "join(...segments)", type: "(...string) => string", desc: "Joins segments with the separator, then normalizes.", ex: `join("src", "lib", "x.js"); // "src/lib/x.js"` },
  { sig: "resolve(...segments)", type: "(...string) => string", desc: "Resolves to an absolute path, anchoring at cwd() if no segment is absolute.", ex: `resolve("data", "out.json"); // "<cwd>/data/out.json"` },
  { sig: "dirname(p)", type: "(string) => string", desc: "The directory portion of p.", ex: `dirname("/var/log/app.log"); // "/var/log"` },
  { sig: "basename(p)", type: "(string) => string", desc: "The final segment of p (no suffix-stripping overload).", ex: `basename("/var/log/app.log"); // "app.log"` },
  { sig: "extname(p)", type: "(string) => string", desc: "The extension of the final segment, including the dot (or empty).", ex: `extname("archive.tar.gz"); // ".gz"` },
  { sig: "parse(p)", type: "(string) => object", desc: "{ root, dir, base, name, ext }.", ex: `parse("/a/b.txt"); // { root, dir, base, name, ext }` },
  { sig: "relative(from, to)", type: "(string, string) => string", desc: "Relative path from from to to (both resolved first).", ex: `relative("/a/b", "/a/c/d"); // "../c/d"` },
  { sig: "fromFileURL(url)", type: "(string | URL) => string", desc: "Converts a file: URL to a path.", ex: `fromFileURL(import.meta.url); // "/abs/mod.js"` },
  { sig: "toFileURL(p)", type: "(string) => URL", desc: "Converts a path (resolved to absolute) to a file: URL.", ex: `toFileURL("/a/b.txt").href; // "file:///a/b.txt"` },
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
            <code className="mt-3 block overflow-x-auto rounded-lg bg-zinc-950 px-3 py-2 font-mono text-[12px] text-emerald-300">
              {e.ex}
            </code>
          </div>
        ))}
      </div>
    </ApiShell>
  );
}
