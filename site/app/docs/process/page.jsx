import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

// Callouts are inlined as plain elements: the framework doubles a child-taking
// custom component when it is nested inside another component's content. To be
// revisited with the MDX migration.

const READ = `import { env, args, cwd } from "runtime:process";

console.log(cwd());     // "/srv/app"
console.log(args);      // ["build", "--watch"]
console.log(env.HOME);  // "/home/app"`;

const ENVFILE = `# .env
DATABASE_URL=postgres://localhost/app
PORT=8080
API_TOKEN=tok_live_123`;

const ENVFILE_RUN = `esrun --env-file .env app.js

# Let the file beat the OS environment (default: OS wins):
esrun --env-file .env --env-override app.js`;

const SECRETS = `import { env, unmask } from "runtime:process";

console.log(env.API_TOKEN);        // [redacted]
console.log(\`\${env.API_TOKEN}\`); // [redacted]
JSON.stringify(env);               // ..."API_TOKEN":"[redacted]"...

const token = unmask(env.API_TOKEN); // real value, explicit`;

export default function ProcessDoc() {
  return (
    <DocsShell active="/docs/process">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Process &amp; Env
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:process</code>{" "}
        gives you the environment, CLI arguments, working directory, and exit.
      </p>
      <p className="mt-3 text-zinc-600">
        It is an ES module, gated on the{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Env</code>{" "}
        capability. Import only what you need.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Reading process info</h2>
      <p className="mt-3 text-zinc-600">
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">env</code> is a
        plain object; <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">args</code>{" "}
        is the script's own arguments.
      </p>
      <div className="mt-4">
        <CodeBlock code={READ} title="app.js" lang="js" />
      </div>
      <div className="mt-5 rounded-xl border border-zinc-200 bg-zinc-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
          Note · In-process only
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          Writing or deleting an <code className="font-mono">env</code> key changes
          it for this run only — it never touches the host or child processes.
        </p>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Environment files</h2>
      <p className="mt-3 text-zinc-600">
        Load variables from a <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.env</code>{" "}
        file with <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">--env-file</code>.
      </p>
      <div className="mt-4">
        <CodeBlock code={ENVFILE} title=".env" lang="sh" />
      </div>
      <div className="mt-4">
        <CodeBlock code={ENVFILE_RUN} title="Terminal" lang="sh" />
      </div>

      <div className="mt-5 rounded-xl border border-amber-200 bg-amber-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-amber-700">
          Warning · No auto-loading
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          A <code className="font-mono">.env</code> is read only when you pass{" "}
          <code className="font-mono">--env-file</code>. Nothing on disk is loaded
          implicitly from the working directory.
        </p>
      </div>
      <div className="mt-5 rounded-xl border border-zinc-200 bg-zinc-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
          Note · Precedence
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          The OS environment wins on a conflict by default, so a committed{" "}
          <code className="font-mono">.env</code> can't clobber production config.
          Pass <code className="font-mono">--env-override</code> to flip it.
        </p>
      </div>
      <div className="mt-5 rounded-xl border border-zinc-200 bg-zinc-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-zinc-500">
          Note · One file
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          A single <code className="font-mono">--env-file</code> is supported —
          production config comes from one <code className="font-mono">.env</code>{" "}
          or the orchestrator. There is no <code className="font-mono">.env.*</code> layering.
        </p>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Secrets are masked</h2>
      <p className="mt-3 text-zinc-600">
        Secret-bearing keys are wrapped so they print as{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">[redacted]</code>{" "}
        in logs, strings, and JSON.
      </p>
      <p className="mt-3 text-zinc-600">
        A key qualifies if it ends in{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">_KEY</code>,{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">_TOKEN</code>,{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">_SECRET</code>,{" "}
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">_PASS(WORD)</code>,
        or contains <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">CREDENTIAL</code>/<code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">AUTH</code>.
      </p>
      <div className="mt-4">
        <CodeBlock code={SECRETS} title="secrets.js" lang="js" />
      </div>
      <div className="mt-5 rounded-xl border border-brand-200 bg-brand-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-brand-700">
          Hint · Reading a secret
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          Call <code className="font-mono">unmask(value)</code> to get the real
          string. Plain values pass through, so{" "}
          <code className="font-mono">unmask(env.ANY)</code> is always safe.
        </p>
      </div>
      <div className="mt-5 rounded-xl border border-rose-200 bg-rose-50 p-4">
        <p className="text-xs font-semibold uppercase tracking-wider text-rose-700">
          Danger · Masking is for accidents, not attackers
        </p>
        <p className="mt-1.5 text-sm leading-relaxed text-zinc-600">
          It stops secrets leaking into logs by mistake. It is not a sandbox — code
          you run can call <code className="font-mono">unmask</code> itself.
        </p>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Exit</h2>
      <p className="mt-3 text-zinc-600">
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">exit(code)</code>{" "}
        records the status and halts immediately — nothing after it runs.
      </p>

      <p className="mt-12 text-sm text-zinc-500">
        Full export list and types:{" "}
        <a href="/api/process" className="font-medium text-brand-600 hover:text-brand-700">
          runtime:process API reference
        </a>
        . CLI flags:{" "}
        <a href="/api/cli" className="font-medium text-brand-600 hover:text-brand-700">
          esrun CLI
        </a>
        .
      </p>
    </DocsShell>
  );
}
