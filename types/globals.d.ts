// Global surface that TypeScript's own lib cannot describe.
//
// The rest of the globals — fetch, streams, URL, crypto — match the standard
// libs closely enough to use as-is. What does not is `Worker`: this runtime
// takes an extra option, because a worker here starts with no capabilities and
// is granted them explicitly.

interface WorkerOptions {
  /**
   * Capabilities to grant the worker, by the same names `--deny-<name>` and
   * `runtime:process` `permissions` use: `"read"`, `"write"`, `"imports"`,
   * `"net"`, `"listen"`, `"env"`, `"run"`, `"signals"`, `"workers"`.
   *
   * A worker starts with **none**, and can never be granted what the agent
   * spawning it does not itself hold — so no chain of spawns widens the
   * original grant. Omitting this yields a worker that can compute and message,
   * and reach nothing.
   *
   * Non-standard, and necessarily so: the HTML `Worker` has no notion of a
   * capability, and a deny-by-default runtime has to say somewhere what a
   * worker may do. Deno spells the equivalent `deno: { permissions }`.
   *
   * ```ts
   * new Worker(new URL("./w.js", import.meta.url), { permissions: ["net"] });
   * ```
   */
  permissions?: readonly string[];

  /**
   * The environment the worker's `runtime:process` `env` reports.
   *
   * - omitted, or `"inherit"` — the host environment, which the worker still
   *   needs the `"env"` permission to read, and which the deployment's
   *   `--allow-env=<names>` still narrows.
   * - an object — **precisely** these variables, and no permission needed to
   *   read them. A parent can only pass values it could already read, so this
   *   attenuates rather than grants. `{}` is a worker with no environment.
   *
   * A handed environment wins over the host's, so a worker holding `"env"` and
   * given one reads what it was given.
   *
   * A `Secret` from the parent's own `env` may be passed straight through: the
   * real value crosses, and the worker re-masks it by the same key convention.
   *
   * ```ts
   * new Worker(url, { env: { DATABASE_URL: unmask(env.DATABASE_URL) } });
   * ```
   *
   * Non-standard, like `permissions`. Node's `SHARE_ENV` has no equivalent here
   * on purpose: a shared, mutable environment is an undeclared side channel
   * between agents, and `postMessage` is the declared one.
   */
  env?: "inherit" | Record<string, string>;
}

interface Navigator {
  /**
   * How many workers can usefully run at once. Reported by the host through
   * the `Process` provider; under `esrun` this respects cgroup and affinity
   * limits, so a container sees its share rather than the whole machine.
   */
  readonly hardwareConcurrency: number;
}

/**
 * The same member, on the interface a *worker's* `navigator` implements.
 * `Navigator` is not exposed inside a worker, and `WorkerNavigator` is not
 * exposed outside one — which interface you have is how the two scopes are told
 * apart.
 */
interface WorkerNavigator {
  readonly hardwareConcurrency: number;
}
