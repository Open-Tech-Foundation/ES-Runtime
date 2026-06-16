import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

const READ = `import { file } from "runtime:fs";

const f = file("./config/app.json");   // lazy — nothing is read yet
const text   = await f.text();          // UTF-8 string
const data   = await f.json();          // parsed JSON
const bytes  = await f.bytes();          // Uint8Array
const buf    = await f.arrayBuffer();    // ArrayBuffer
const rs     = f.stream();               // ReadableStream
const ok     = await f.exists();         // boolean
const info   = await f.stat();           // { size, isFile, isDir, mtimeMs }`;

const WRITE = `import { write } from "runtime:fs";

await write("./out/result.txt", "done");                  // string
await write("/srv/app/cache.bin", new Uint8Array([1, 2])); // bytes`;

const APPEND = `import { write } from "runtime:fs";

await write("./app.log", "started\\n", { append: true });  // append`;

const STREAM = `import { file } from "runtime:fs";

// Any web body works — and you can stream straight to disk:
const res = await fetch("https://example.com/big.bin");
await res.body.pipeTo(file("./big.bin").writable());`;

const CREATE_DIR = `import { mkdir } from "runtime:fs";

await mkdir("./logs/2026", { recursive: true });`;

const READ_DIR = `import { readDir } from "runtime:fs";

for (const entry of await readDir("./logs")) {
  console.log(entry.name, entry.isDir);
}`;

const RENAME = `import { rename } from "runtime:fs";

await rename("./logs/app.log", "./logs/app.1.log");`;

const REMOVE = `import { remove } from "runtime:fs";

await remove("./logs", { recursive: true });`;

export default function FileHandlingGuide() {
  return (
    <DocsShell active="/docs/guides/file-handling">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        File handling
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Read and write files with{" "}
        <a href="/api/fs" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">runtime:fs</code>
        </a>{" "}
        — a Blob-based surface, gated by capabilities and confined to the project
        root jail.
      </p>

      <div className="mt-6 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Tip.</strong> Every operation is async —
        use <code className="font-mono">await</code>. Top-level{" "}
        <code className="font-mono">await</code> covers the “load config at
        startup” case.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Reading files</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">file(path)</code> is a lazy handle; pick the
        read shape you want.
      </p>
      <div className="mt-4">
        <CodeBlock code={READ} title="read.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Write file</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">write()</code> accepts any web body (string, bytes, etc.).
      </p>
      <div className="mt-4">
        <CodeBlock code={WRITE} title="write.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Append file</h2>
      <p className="mt-3 text-zinc-600">
        Pass <code className="font-mono">{"{ append: true }"}</code> to add to the end of the file.
      </p>
      <div className="mt-4">
        <CodeBlock code={APPEND} title="append.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Stream to write</h2>
      <p className="mt-3 text-zinc-600">
        You can stream data straight to disk using standard web streams.
      </p>
      <div className="mt-4">
        <CodeBlock code={STREAM} title="stream.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-bold text-zinc-900">Folders</h2>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Create directory</h3>
      <div className="mt-4">
        <CodeBlock code={CREATE_DIR} title="mkdir.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Read directory</h3>
      <div className="mt-4">
        <CodeBlock code={READ_DIR} title="readdir.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Rename</h3>
      <div className="mt-4">
        <CodeBlock code={RENAME} title="rename.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Remove</h3>
      <div className="mt-4">
        <CodeBlock code={REMOVE} title="remove.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Capabilities & the jail</h2>
      <p className="mt-3 text-zinc-600">
        Reads need <code className="font-mono">FileRead</code>, writes need{" "}
        <code className="font-mono">FileWrite</code> (the CLI grants both), and
        every path is confined to the project root — escapes via{" "}
        <code className="font-mono">..</code> or a symlink are rejected.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">How other runtimes compare</h2>
      <ul className="mt-4 space-y-2 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Node.js</strong> — <code className="font-mono">node:fs</code>{" "}
            in callback, sync, and promise flavors.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Deno</strong> — promise-based{" "}
            <code className="font-mono">Deno.*</code> behind <code className="font-mono">--allow-read</code>/<code className="font-mono">--allow-write</code>.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Bun</strong> — Blob-based{" "}
            <code className="font-mono">Bun.file</code>/<code className="font-mono">Bun.write</code>, the shape esrun follows.
          </span>
        </li>
      </ul>
    </DocsShell>
  );
}
