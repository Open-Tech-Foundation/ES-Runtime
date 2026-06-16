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

// Match specific extensions
const webFiles = new Glob("**/*.{html,css,js}");

// Exclude directories
const nonModules = new Glob("**/*.js", { exclude: ["**/node_modules/**"] });

// Cross-OS behavior:
// Glob paths always use '/' separators, even on Windows.
// The matching is consistent across environments.
const winGlob = new Glob("C:/project/**/*.ts");
`;

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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Useful patterns & Cross-OS</h2>
      <p className="mt-3 text-zinc-600">
        Support for wildcards (<code className="font-mono">*</code>, <code className="font-mono">**</code>) and groups (<code className="font-mono">{"{a,b}"}</code>).
      </p>
      
      <div className="mt-6 rounded-lg border border-zinc-200 bg-zinc-50 px-4 py-3 text-sm leading-relaxed text-zinc-600">
        <strong className="text-zinc-900">Info:</strong> Path separators are always treated as <code className="font-mono">/</code> for consistency across Linux, macOS, and Windows.
      </div>
      <div className="mt-4">
        <CodeBlock code={GLOB_PATTERNS} title="patterns.js" lang="js" />
      </div>

    </DocsShell>
  );
}
