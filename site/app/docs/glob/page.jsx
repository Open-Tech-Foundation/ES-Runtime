import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const GLOB_BASIC = `import { Glob } from "runtime:fs";

const ts = new Glob("**/*.ts");
ts.match("src/index.ts"); // true (pure, no I/O)

// Scan a directory recursively for matches
for await (const path of ts.scan("src")) {
  console.log(path);
}`;

const GLOB_PATTERNS = `import { Glob } from "runtime:fs";

const js = new Glob("**/*.js");
for await (const path of js.scan("src")) {   // walks only ./src
  if (path.includes("/vendor/")) continue;   // ...and filter in JS
  console.log(path);
}`;

// Every token a Glob pattern supports, with a copy-able example.
const PATTERNS = [
  { token: "*", desc: "Any run of characters within one path segment (not '/').", eg: '"*.ts"' },
  { token: "**", desc: "Any characters, crossing '/' — recurse into subdirectories.", eg: '"src/**/*.ts"' },
  { token: "?", desc: "Exactly one character (not '/').", eg: '"v?.json"' },
  { token: "[abc]", desc: "Any one character in the set.", eg: '"[abc]*.js"' },
  { token: "[a-z]", desc: "Any one character in the range.", eg: '"[0-9]*.log"' },
  { token: "[!abc]", desc: "Any one character NOT in the set ([^abc] works too).", eg: '"[!_]*.ts"' },
  { token: "{a,b}", desc: "Alternation — match any of the comma-separated options.", eg: '"*.{ts,tsx}"' },
  { token: "\\", desc: "Escape — match the next metacharacter literally.", eg: '"file\\*.txt"' },
  { token: "!…", desc: "A leading '!' negates the whole pattern.", eg: '"!**/*.test.ts"' },
];

export default function GlobGuide() {
  return (
    <DocsShell active="/docs/glob">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Glob matching
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Find files and match paths using the{" "}
        <a href="/api/fs" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">Glob</code>
        </a>{" "}
        API from <code className="font-mono">runtime:fs</code>.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Basic matching & scanning</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">Glob</code> matches strings without I/O, or it can scan the
        filesystem tree.
      </p>
      <div className="mt-4">
        <CodeBlock code={GLOB_BASIC} title="glob.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Pattern syntax</h2>
      <p className="mt-3 text-zinc-600">
        Every token a <code className="font-mono">Glob</code> pattern supports:
      </p>
      <div className="mt-4 overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-sm">
          <tbody className="divide-y divide-zinc-100">
            {PATTERNS.map((p) => (
              <tr>
                <td className="w-24 px-4 py-2.5 align-top font-mono font-medium text-brand-700">{p.token}</td>
                <td className="px-4 py-2.5 text-zinc-600">
                  {p.desc}{" "}
                  <code className="ml-1 whitespace-nowrap font-mono text-zinc-400">{p.eg}</code>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="mt-6 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Cross-OS:</strong> patterns and matched paths always use <code className="font-mono">/</code> separators, even on Windows — matching is identical on Linux, macOS, and Windows.
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Narrowing a scan</h2>
      <p className="mt-3 text-zinc-600">
        There's no <code className="font-mono">exclude</code> option — scan a narrower
        root to skip a subtree, or filter the results yourself.
      </p>
      <div className="mt-4">
        <CodeBlock code={GLOB_PATTERNS} title="scan.js" lang="js" />
      </div>

    </DocsShell>
  );
}
