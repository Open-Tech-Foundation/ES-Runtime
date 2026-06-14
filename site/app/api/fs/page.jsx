import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const IMPORT = `import { file, write, readDir, stat, mkdir, remove } from "runtime:fs";

await mkdir("data", { recursive: true });
await write("data/app.json", JSON.stringify({ ok: true }));

const f = file("data/app.json");   // lazy, Blob-like handle
const cfg = await f.json();         // .text() / .bytes() / .arrayBuffer() / .stream()
await write("data/copy.json", f);   // any web body works`;

const fns = [
  { sig: "file(path)", type: "(path) => FsFile", desc: "A lazy, Blob-like handle — nothing is read until a read method is called." },
  { sig: "write(dest, input)", type: "(path, body) => Promise<number>", desc: "Writes any web body (string | Blob | ArrayBuffer | TypedArray | Response | ReadableStream | file()) to dest; resolves to bytes written. Streams to disk if given a ReadableStream/Response." },
  { sig: "readDir(path)", type: "(path) => Promise<DirEntry[]>", desc: "Directory entries: { name, isFile, isDir, isSymlink }." },
  { sig: "stat(path)", type: "(path) => Promise<Stat>", desc: "{ size, isFile, isDir, isSymlink, mtimeMs } (follows symlinks)." },
  { sig: "exists(path)", type: "(path) => Promise<boolean>", desc: "Whether the path exists (missing is false, not an error)." },
  { sig: "mkdir(path, opts?)", type: "(path, { recursive? }) => Promise<void>", desc: "Creates a directory; recursive creates parents." },
  { sig: "remove(path, opts?)", type: "(path, { recursive? }) => Promise<void>", desc: "Removes a file or (with recursive) a directory tree." },
  { sig: "rename(from, to)", type: "(path, path) => Promise<void>", desc: "Renames/moves an entry (both jailed)." },
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
        surface — lazy file handles and writes that accept any web body. Every
        operation is async, and every path is confined to the project root jail.
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

      <p className="mt-8 text-zinc-600">
        <code className="font-mono">file()</code> is a lazy, Blob-like handle
        with the web read surface; <code className="font-mono">write()</code>{" "}
        takes any web body. Paths may be a string, a{" "}
        <code className="font-mono">file:</code> URL, or a{" "}
        <code className="font-mono">file()</code> handle. Reads need{" "}
        <code className="font-mono">FileRead</code>, mutations need{" "}
        <code className="font-mono">FileWrite</code>; a path that escapes the root
        jail (via <code className="font-mono">..</code> or a symlink) is rejected.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Import</h2>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="runtime:fs" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Module functions
      </h2>
      <div className="mt-5 space-y-4">
        {fns.map((e) => (
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        FsFile (from file(path))
      </h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">text()</code>,{" "}
        <code className="font-mono">json()</code>,{" "}
        <code className="font-mono">bytes()</code> (Uint8Array),{" "}
        <code className="font-mono">arrayBuffer()</code>,{" "}
        <code className="font-mono">stream()</code> (ReadableStream),{" "}
        <code className="font-mono">exists()</code>,{" "}
        <code className="font-mono">stat()</code>,{" "}
        <code className="font-mono">write(data)</code>,{" "}
        <code className="font-mono">delete()</code>, and the{" "}
        <code className="font-mono">path</code> it points at.
      </p>
    </ApiShell>
  );
}
