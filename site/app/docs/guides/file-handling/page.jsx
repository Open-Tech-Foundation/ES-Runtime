import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

const BASICS = `import { file, write, readDir } from "runtime:fs";

// A handle is lazy — nothing is read until you ask.
const f = file("./config/app.json");
if (await f.exists()) {
  const cfg = await f.json();          // .text() / .bytes() / .arrayBuffer() / .stream()
}

// write() takes any web body: string, Blob, ArrayBuffer, TypedArray,
// Response, ReadableStream, or another file().
await write("/srv/app/cache.bin", new Uint8Array([1, 2, 3]));
await write("./out/log.txt", "started\\n", { append: true });`;

const DIRNAME = `import { dirname, join, fromFileURL } from "runtime:path";

// The modern __dirname — derive paths relative to the current module.
const here = dirname(fromFileURL(import.meta.url));
const data = join(here, "fixtures", "input.csv");`;

const PORTABLE = `import { join } from "runtime:path";

// Portable: join() uses the host separator (/ on POSIX, \\ on Windows).
const p = join("logs", "2026", "app.log");   // logs/app.log shape, OS-correct

// file: URLs work as inputs too (handy with import.meta.url):
import { file } from "runtime:fs";
const self = await file(import.meta.url).text();`;

const PIPE = `import { file } from "runtime:fs";

// Stream a download straight to disk — no buffering in memory.
const res = await fetch("https://example.com/big.bin");
await res.body.pipeTo(file("./big.bin").writable());`;

export default function FileHandlingGuide() {
  return (
    <DocsShell active="/docs/guides/file-handling">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        File handling
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun reads and writes files through the{" "}
        <a href="/api/fs" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">runtime:fs</code>
        </a>{" "}
        module. It is built on the web <code className="font-mono">Blob</code>{" "}
        surface, gated by capabilities, and confined to a project root jail.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Four things to know</h2>
      <ul className="mt-4 space-y-3 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Everything is async.</strong> There
            are no synchronous variants. esrun is a driven runtime with no thread
            of its own, so blocking on disk would stall the whole event loop —
            every read and write returns a <code className="font-mono">Promise</code>.
            Top-level <code className="font-mono">await</code> covers the
            “load config at startup” case.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Capabilities gate access.</strong>{" "}
            Reads need <code className="font-mono">FileRead</code>, writes need{" "}
            <code className="font-mono">FileWrite</code>. The standalone{" "}
            <code className="font-mono">esrun</code> CLI grants both; an embedder
            grants them explicitly (the embeddable library is deny-by-default).
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Paths are jailed.</strong> Every
            path is resolved to its real location and confined to the project
            root; a path that escapes via <code className="font-mono">..</code> or
            a symlink is rejected.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-brand-500" />
          <span>
            <strong className="text-zinc-900">Handles are Blob-like.</strong>{" "}
            <code className="font-mono">file(path)</code> is lazy;{" "}
            <code className="font-mono">write()</code> accepts any web body.
          </span>
        </li>
      </ul>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Reading and writing</h2>
      <div className="mt-4">
        <CodeBlock code={BASICS} title="basics.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Paths across operating systems</h2>
      <p className="mt-3 text-zinc-600">
        Separators differ — <code className="font-mono">/</code> on Linux and
        macOS, <code className="font-mono">\\</code> (plus drive letters like{" "}
        <code className="font-mono">C:\\</code>) on Windows. Build paths with{" "}
        <a href="/api/path" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">runtime:path</code>
        </a>{" "}
        rather than string concatenation so they stay correct everywhere; it
        takes the real OS from <code className="font-mono">runtime:process</code>.
      </p>
      <div className="mt-4">
        <CodeBlock code={PORTABLE} title="portable.js" lang="js" />
      </div>
      <p className="mt-4 text-zinc-600">
        To locate files relative to the current module — the modern replacement
        for Node’s <code className="font-mono">__dirname</code> — combine{" "}
        <code className="font-mono">fromFileURL</code> with{" "}
        <code className="font-mono">import.meta.url</code>:
      </p>
      <div className="mt-4">
        <CodeBlock code={DIRNAME} title="here.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Streaming to disk</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">file(path).writable()</code> is a web-standard{" "}
        <code className="font-mono">WritableStream</code>, so you can pipe a
        response body straight to a file without buffering it in memory.
      </p>
      <div className="mt-4">
        <CodeBlock code={PIPE} title="download.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">How other runtimes compare</h2>
      <p className="mt-3 text-zinc-600">
        File I/O is not part of the web platform, so each runtime designs its own
        surface — and they have converged on similar ideas:
      </p>
      <ul className="mt-4 space-y-2 text-zinc-600">
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Node.js</strong> offers{" "}
            <code className="font-mono">node:fs</code> in three flavors —
            callbacks, sync, and the promise API — plus a permissionless default.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Deno</strong> exposes promise-based{" "}
            <code className="font-mono">Deno.*</code> calls and gates filesystem
            access behind <code className="font-mono">--allow-read</code>/
            <code className="font-mono">--allow-write</code>.
          </span>
        </li>
        <li className="flex gap-3">
          <span className="mt-2 h-1.5 w-1.5 shrink-0 rounded-full bg-zinc-300" />
          <span>
            <strong className="text-zinc-900">Bun</strong> models files as Blobs
            (<code className="font-mono">Bun.file</code>/<code className="font-mono">Bun.write</code>),
            which is the shape esrun follows.
          </span>
        </li>
      </ul>
      <p className="mt-4 text-zinc-600">
        esrun’s take pairs that Blob-based ergonomics with a capability check and
        a root jail on every operation, and stays async-only to fit a runtime the
        host drives.
      </p>
    </DocsShell>
  );
}
