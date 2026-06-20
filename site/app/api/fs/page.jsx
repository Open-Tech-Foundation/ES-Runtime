import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import ErrorTable from "../../../components/ErrorTable.jsx";

const errors = [
  { e: "TypeError", w: "A path isn't a string, URL, or file() handle; a non-file: URL is passed; or write() gets an unsupported input type." },
  { e: "DOMException", w: "name \"NotAllowedError\" — the FileRead or FileWrite capability is not granted." },
  { e: "Error", w: "A filesystem failure surfaced by the OS (not found, permission denied, etc.)." },
];

const IMPORT = `import { file, write, readDir, stat, mkdir, remove, rename, Glob } from "runtime:fs";`;

const fns = [
  {
    sig: "file(path)",
    type: "(path: PathLike) => FsFile",
    desc: "A lazy, Blob-like handle — nothing is read until a read method is called.",
    ex: `const f = file("./config/app.json");`,
  },
  {
    sig: "write(dest, input, options?)",
    type: "(dest, body, { append? }) => Promise<number>",
    desc: "Writes any web body (string | Blob | ArrayBuffer | TypedArray | Response | ReadableStream | file()) to dest; resolves to bytes written.",
    ex: `await write("/srv/app/out.bin", bytes, { append: true });`,
  },
  {
    sig: "readDir(path)",
    type: "(path) => Promise<DirEntry[]>",
    desc: "Directory entries: { name, isFile, isDir, isSymlink }.",
    ex: `for (const e of await readDir("./src")) console.log(e.name);`,
  },
  {
    sig: "stat(path)",
    type: "(path) => Promise<Stat>",
    desc: "{ size, isFile, isDir, isSymlink, mtimeMs } — follows symlinks.",
    ex: `const { size } = await stat("C:\\\\data\\\\cache.bin");`,
  },
  {
    sig: "exists(path)",
    type: "(path) => Promise<boolean>",
    desc: "Whether the path exists (a missing path is false, not an error).",
    ex: `if (await exists("./.cache")) { /* … */ }`,
  },
  {
    sig: "mkdir(path, options?)",
    type: "(path, { recursive? }) => Promise<void>",
    desc: "Creates a directory; recursive creates missing parents.",
    ex: `await mkdir("./logs/2026", { recursive: true });`,
  },
  {
    sig: "remove(path, options?)",
    type: "(path, { recursive? }) => Promise<void>",
    desc: "Removes a file or (with recursive) a directory tree.",
    ex: `await remove("./tmp", { recursive: true });`,
  },
  {
    sig: "rename(from, to)",
    type: "(from, to) => Promise<void>",
    desc: "Renames or moves an entry (both jailed).",
    ex: `await rename("./draft.md", "./final.md");`,
  },
  {
    sig: "new Glob(pattern)",
    type: "Glob",
    desc: "Glob matcher/scanner. match(path) is pure; scan(cwd | options) is an async iterator over the jailed tree. Patterns: *, **, ?, [a-z], [!x], {a,b}, leading !.",
    ex: `new Glob("**/*.ts").match("src/app.ts"); // true`,
  },
];

const members = [
  { m: "path", t: "string", d: "The path this handle points at." },
  { m: "text()", t: "Promise<string>", d: "Read the whole file as UTF-8 text." },
  { m: "json()", t: "Promise<any>", d: "Read and JSON.parse the file." },
  { m: "bytes()", t: "Promise<Uint8Array>", d: "Read the whole file as bytes." },
  { m: "arrayBuffer()", t: "Promise<ArrayBuffer>", d: "Read the whole file as an ArrayBuffer." },
  { m: "stream()", t: "ReadableStream", d: "A readable byte stream of the file." },
  { m: "exists()", t: "Promise<boolean>", d: "Whether the file exists." },
  { m: "stat()", t: "Promise<Stat>", d: "File metadata (follows symlinks)." },
  { m: "write(data, options?)", t: "Promise<number>", d: "Write to this file; resolves to bytes written." },
  { m: "writable(options?)", t: "WritableStream", d: "A sink for piped/incremental writes. First chunk truncates unless { append: true } is set, rest append." },
  { m: "delete()", t: "Promise<void>", d: "Delete this file." },
];

export default function FsDoc() {
  return (
    <ApiShell active="/api/fs">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:fs
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Blob-based file I/O, modeled on the web <code className="font-mono">Blob</code>{" "}
        surface — lazy file handles and writes that accept any web body.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: FileRead / FileWrite
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · runtime: scheme
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
      </div>

      <div className="mt-6 flex items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 p-4 text-sm leading-relaxed text-amber-900">
        <span className="mt-0.5 shrink-0 font-semibold">async</span>
        <span>
          <strong>Every operation returns a Promise.</strong> There are no
          synchronous variants — esrun is a driven runtime with no thread of its
          own, so file I/O never blocks the event loop. Use{" "}
          <code className="font-mono">await</code> (top-level{" "}
          <code className="font-mono">await</code> works). Paths are a string, a{" "}
          <code className="font-mono">file:</code> URL, or a{" "}
          <code className="font-mono">file()</code> handle, and every path is
          confined to the project root jail.
        </span>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Import</h2>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="runtime:fs" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Functions</h2>
      <div className="mt-5 space-y-4">
        {fns.map((e) => (
          <div className="rounded-xl border border-zinc-200 p-5">
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
              <code className="font-mono text-[15px] font-semibold text-zinc-900">
                {e.sig}
              </code>
              <code className="font-mono text-[13px] text-zinc-400">{e.type}</code>
            </div>
            <p className="mt-2 text-sm leading-relaxed text-zinc-600">{e.desc}</p>
            <code className="mt-3 block overflow-x-auto rounded-lg bg-zinc-950 px-3 py-2 font-mono text-[12px] text-emerald-300">
              {e.ex}
            </code>
          </div>
        ))}
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        FsFile — the <code className="font-mono">file(path)</code> handle
      </h2>
      <div className="mt-5 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
            <tr>
              <th className="px-4 py-3 font-semibold">Member</th>
              <th className="px-4 py-3 font-semibold">Returns</th>
              <th className="px-4 py-3 font-semibold">Description</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-100">
            {members.map((x) => (
              <tr>
                <td className="px-4 py-3 font-mono text-[13px] font-medium text-zinc-900">
                  {x.m}
                </td>
                <td className="px-4 py-3 font-mono text-[13px] text-zinc-500">{x.t}</td>
                <td className="px-4 py-3 text-zinc-600">{x.d}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Errors</h2>
      <ErrorTable rows={errors} />
    </ApiShell>
  );
}
