// Global surface that TypeScript's own lib cannot describe.
//
// The rest of the globals — fetch, streams, URL, crypto — match the standard
// libs closely enough to use as-is. What does not is `Worker`: this runtime
// takes an extra option, because a worker here starts with no capabilities and
// is granted them explicitly.

interface WorkerOptions {
  /**
   * Capabilities to grant the worker.
   *
   * - omitted — **none**. A worker starts confined: it can compute and message,
   *   and reach nothing.
   * - an array — exactly these.
   * - `"inherit"` — everything the spawning agent holds.
   *
   * A worker can never be granted what the agent spawning it does not itself
   * hold, so no chain of spawns widens the original grant — `"inherit"` is a
   * ceiling, not an escape. An unknown name throws rather than being skipped: a
   * dropped typo would leave the worker taking the degraded path forever.
   *
   * Non-standard, and necessarily so: the HTML `Worker` has no notion of a
   * capability, and a deny-by-default runtime has to say somewhere what a
   * worker may do. Deno spells the equivalent `deno: { permissions }`.
   *
   * ```ts
   * new Worker(new URL("./w.js", import.meta.url), { permissions: ["net"] });
   * ```
   */
  permissions?: "inherit" | readonly import("runtime:process").PermissionName[];

  /**
   * The worker's heap ceiling, in **megabytes** — the unit Node's
   * `resourceLimits.maxOldGenerationSizeMb` uses.
   *
   * Omitted, the worker takes the ceiling of the agent that started it (set by
   * `--max-heap=<mb>`, and by default sized from the container's memory limit
   * or the host's memory). Named, it may only **lower** that; asking for more
   * throws.
   *
   * Reaching it ends that worker and no other: the parent's `error` fires with
   * `e.error.name === "ERR_WORKER_OUT_OF_MEMORY"`.
   *
   * ```ts
   * new Worker(url, { memory: 64 });
   * ```
   */
  memory?: number;

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

interface Worker {
  /**
   * Stop this worker from keeping the process alive.
   *
   * It carries on running and still delivers messages — the only thing given
   * up is being a reason not to exit. For a pool, idle workers waiting for the
   * next job would otherwise hold the process open forever.
   *
   * Non-standard; Node and Bun both have it, Deno has neither.
   *
   * ```ts
   * const w = new Worker(url);
   * w.unref();          // still works; no longer holds the process open
   * ```
   */
  unref(): void;

  /**
   * Undoes {@link Worker.unref}. A worker starts referenced, so this only
   * matters after an `unref()`.
   */
  ref(): void;

  /**
   * How many messages have been posted to this worker and not yet taken by it.
   *
   * The only backpressure signal there is: `postMessage` never refuses a
   * message — HTML does not permit it to fail for queue depth, and Node, Deno
   * and Bun all queue without limit — so a producer that outruns its worker
   * grows memory unless it chooses to pace itself. This is what it paces
   * against, the way a socket's `bufferedAmount` works.
   *
   * ```ts
   * for (const job of jobs) {
   *   w.postMessage(job);
   *   if (w.queued > 1000) await drain();
   * }
   * ```
   *
   * Non-standard. No other runtime exposes this.
   */
  readonly queued: number;
}

interface DedicatedWorkerGlobalScope {
  /**
   * How many messages this worker has sent to its parent and the parent has
   * not yet taken — the mirror of {@link Worker.queued}, for a worker
   * producing results faster than its parent consumes them.
   */
  readonly queued: number;
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
