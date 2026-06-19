import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const IMPORT = `import { env, args, platform, arch, cwd, exit, unmask } from "runtime:process";

// Or the default aggregate:
import process from "runtime:process";`;

const SECRET_EX = `import { env, unmask } from "runtime:process";

// Keys ending in *_SECRET(S) / *_PASSWORD(S) are masked by default.
console.log(env.DB_PASSWORD);        // [redacted]
console.log(\`\${env.DB_PASSWORD}\`); // [redacted]
JSON.stringify(env);                 // ..."DB_PASSWORD":"[redacted]"...

const pw = unmask(env.DB_PASSWORD);  // real value, explicit
unmask(env.DB_HOST);                 // plain strings pass through`;

const ENV_EX = `import { env } from "runtime:process";

console.log(env.HOME);      // read
env.FEATURE_FLAG = "on";    // write (in-process only)
delete env.SECRET;          // delete (in-process only)`;

const ARGS_EX = `// esrun app.js build --watch
import { args } from "runtime:process";

console.log(args); // ["build", "--watch"]`;

const EXIT_EX = `import { exit } from "runtime:process";

if (failed) exit(1); // records the code and halts immediately
exit();              // defaults to 0`;

const exports = [
  {
    sig: "env",
    type: "Record<string, string | Secret>",
    desc: "Environment variables as a mutable in-process object, seeded from a host snapshot taken when the module is evaluated (plus any --env-file values). Reads, writes, and deletes work in-process; they do not propagate to the host or to child processes. Secret-keyed values are Secret wrappers (see below).",
    ex: `env.HOME; env.FLAG = "on"; delete env.CACHE;`,
  },
  {
    sig: "args",
    type: "readonly string[]",
    desc: "Program arguments after the runtime binary and the script (or -e snippet). Frozen. Excludes the executable and script path.",
    ex: `args; // ["build", "--watch"]`,
  },
  {
    sig: "platform",
    type: "string",
    desc: "Host operating system — the OS-native std value.",
    ex: `platform; // "linux" | "macos" | "windows"`,
  },
  {
    sig: "arch",
    type: "string",
    desc: "Host CPU architecture — the OS-native std value.",
    ex: `arch; // "x86_64" | "aarch64" | "arm"`,
  },
  {
    sig: "cwd()",
    type: "() => string",
    desc: "Current working directory. A function (not a value) because it can change during a run.",
    ex: `cwd(); // "/srv/app"`,
  },
  {
    sig: "exit(code = 0)",
    type: "(code?: number) => never",
    desc: "Records the exit code and halts execution immediately — code after the call does not run. The embedder treats it as a clean exit, not an error.",
    ex: `if (failed) exit(1); // halts immediately`,
  },
  {
    sig: "unmask(value)",
    type: "(value: string | Secret) => string",
    desc: "Reveal the real value of a masked Secret. A plain string is returned unchanged, so unmask(env.ANY) is always safe.",
    ex: `unmask(env.DB_PASSWORD); // real value`,
  },
  {
    sig: "Secret",
    type: "class",
    desc: "Opaque holder for a masked env value (keys ending in *_SECRET(S) / *_PASSWORD(S)). Renders as \"[redacted]\" in console, string coercion, and JSON. Call unmask() to read it.",
    ex: `env.API_SECRET instanceof Secret; // true`,
  },
];

export default function ProcessDoc() {
  return (
    <ApiShell active="/api/process">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:process
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Host process information: environment, arguments, working directory,
        platform, and exit. Aligned in spirit with the WinterTC CLI-API
        proposal.
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Import</h2>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="runtime:process" lang="js" />
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        env — reading and writing
      </h2>
      <div className="mt-4">
        <CodeBlock code={ENV_EX} title="env.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        args — program arguments
      </h2>
      <div className="mt-4">
        <CodeBlock code={ARGS_EX} title="args.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        exit — stopping the run
      </h2>
      <div className="mt-4">
        <CodeBlock code={EXIT_EX} title="exit.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">
        Secrets — masked by default
      </h2>
      <p className="mt-4 text-sm leading-relaxed text-zinc-600">
        Env entries whose key ends in{" "}
        <code className="font-mono">*_SECRET(S)</code> or{" "}
        <code className="font-mono">*_PASSWORD(S)</code> (case-insensitive) are
        exposed as a <code className="font-mono">Secret</code> that renders as{" "}
        <code className="font-mono">[redacted]</code> in console output, string
        coercion / template literals, and{" "}
        <code className="font-mono">JSON.stringify</code> — so they don't leak
        into logs by accident. Call <code className="font-mono">unmask()</code>{" "}
        to read the value. This guards against accidental disclosure, not a
        hostile guest (which can call <code className="font-mono">unmask</code>{" "}
        itself).
      </p>
      <div className="mt-4">
        <CodeBlock code={SECRET_EX} title="secrets.js" lang="js" />
      </div>
      <p className="mt-4 text-sm leading-relaxed text-zinc-600">
        Load values from a file with{" "}
        <a href="/api/cli" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">esrun --env-file .env</code>
        </a>{" "}
        (no auto-discovery; the OS environment wins unless{" "}
        <code className="font-mono">--env-override</code> is passed).
      </p>

      <div className="mt-12 rounded-xl border border-zinc-200 bg-zinc-50 p-5 text-sm text-zinc-600">
        <strong className="text-zinc-900">Note.</strong> The default export is an
        object bundling all named exports — useful for a single import binding —
        but named imports are preferred for clarity and tree-shaking.
      </div>
    </ApiShell>
  );
}
