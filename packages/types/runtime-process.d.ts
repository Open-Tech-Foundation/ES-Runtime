declare module "runtime:process" {
  /**
   * An opaque holder for a secret env value. Env entries with a secret-bearing
   * key (case-insensitive) — ending in `_KEY(S)`, `_TOKEN(S)`, `_SECRET(S)`,
   * `_PASS`, or `_PASSWORD(S)`, or containing `CREDENTIAL`/`AUTH` — are exposed
   * as a `Secret` instead of a raw string, so they render as `"[redacted]"`
   * everywhere they
   * would otherwise leak — console output, string coercion / template literals,
   * and `JSON.stringify`. Call {@link unmask} to obtain the real value.
   */
  export class Secret {
    private constructor();
    toString(): string;
    toJSON(): string;
  }

  /**
   * Environment variables as a mutable in-process object, seeded from a host
   * snapshot taken when the module is evaluated. Reads, writes, and deletes work
   * in-process; they do not propagate to the host process or to child processes.
   *
   * Assigned values are coerced to strings, since an environment holds nothing
   * else — `env.PORT = 8080` stores `"8080"`. A symbol has no string value and
   * throws, as it does in Node and Deno.
   *
   * Values for secret-bearing keys (e.g. `*_KEY`, `*_TOKEN`, `*_SECRET`,
   * `*_PASSWORD`, `*CREDENTIAL*`, `*AUTH*`) are {@link Secret} wrappers; pass
   * them through {@link unmask} to read the value.
   */
  export const env: Record<string, string | Secret>;

  /**
   * Reveal the real value of a {@link Secret}. A plain `string` is returned
   * unchanged, so `unmask(env.ANY_KEY)` is always safe.
   */
  export function unmask(value: string | Secret): string;

  /**
   * Program arguments after the runtime binary and the script (or `-e` snippet).
   * Frozen; excludes the executable and script path.
   */
  export const args: readonly string[];

  /** Host operating system — the OS-native value (`"linux"`, `"macos"`, `"windows"`, …). */
  export const platform: string;

  /** Host CPU architecture — the OS-native value (`"x86_64"`, `"aarch64"`, `"arm"`, …). */
  export const arch: string;

  /** The current working directory (a function — it can change during a run). */
  export function cwd(): string;

  /**
   * Records the exit code and halts execution immediately — code after the call
   * does not run.
   */
  export function exit(code?: number): never;

  /** A signal name this runtime understands. */
  export type SignalName = "SIGINT" | "SIGTERM" | "SIGHUP" | "SIGUSR1" | "SIGUSR2" | "SIGBREAK";

  /**
   * The signal names this platform can actually deliver: `SIGINT`, `SIGTERM`,
   * `SIGHUP`, `SIGUSR1` and `SIGUSR2` on Unix; `SIGINT` and `SIGBREAK` on
   * Windows. Needs the `Signals` capability, but watches nothing.
   */
  export function signals(): SignalName[];

  /**
   * Run `handler` when `signal` arrives. The first handler for a signal starts
   * watching it, which **suppresses its default action** — so a `SIGTERM`
   * handler is what stops the process being killed outright, and is how a
   * graceful shutdown is written.
   *
   * While anything is watched the program stays alive to receive it (as in Node
   * and Deno); removing the last handler with {@link offSignal} releases it.
   *
   * Needs the `Signals` capability. Throws if `signal` is not a name this
   * runtime knows, or is one this platform cannot deliver — a handler that
   * could never fire is worse than a clear failure.
   */
  export function onSignal(signal: SignalName, handler: (signal: SignalName) => void): void;

  /**
   * Remove a handler added with {@link onSignal}. Removing the last handler for
   * a signal stops watching it and restores the default action.
   */
  export function offSignal(signal: SignalName, handler: (signal: SignalName) => void): void;

  /**
   * A capability this process may hold. These are exactly the suffixes of the
   * `--allow-<name>` / `--deny-<name>` flags:
   *
   * - `read` / `write` — the `runtime:fs` and `runtime:wasi` surfaces
   * - `imports` — the module loader (`import "./x.js"`, `import "pkg"`)
   * - `net` / `listen` — outbound (`fetch`, `WebSocket`, `runtime:net`) and
   *   inbound (`runtime:net` listen, `runtime:http` serve)
   * - `env` — this module's `env` and `cwd()` (`args` needs no grant: it is the
   *   command line that started this program)
   * - `run` — `runtime:system` child processes
   * - `signals` — `onSignal`
   * - `workers` — starting a `Worker`
   */
  export type PermissionName =
    | "read"
    | "write"
    | "imports"
    | "net"
    | "listen"
    | "env"
    | "run"
    | "signals"
    | "workers";

  /**
   * What this process is allowed to reach. The policy is fixed at launch — by
   * the CLI's `--allow-<name>` / `--deny-<name>` flags, or by the embedder's
   * capability set — so this is introspection only: there is nothing to request
   * and no prompt to await, which is why it is synchronous.
   *
   * `esrun` grants nothing unless the command line said so, so a program that
   * adapts to what it holds should expect to hold little. This needs no
   * capability itself: it answers with everything denied, which is the policy
   * under which a program most needs to ask.
   *
   * ```js
   * import { permissions } from "runtime:process";
   * if (permissions.has("write")) await fs.write("cache.json", data);
   * ```
   */
  export const permissions: {
    /** The names this process may not use — empty when nothing is denied. */
    readonly denied: readonly PermissionName[];
    /**
     * Whether `name` is available. Throws a `TypeError` for a name outside
     * {@link PermissionName}, rather than answering `false` — a typo'd check
     * would otherwise read as a denial and take the degraded path forever.
     *
     * A **scoped** grant (`--allow-env=PORT`, `--allow-read=./data`,
     * `--allow-net=api.example.com`, …) answers `true`: the capability is
     * granted, and the list of paths, addresses, programs or variables it was
     * narrowed to is enforced by the host when you use it. So `true` means
     * "you may read *an* environment variable", not "you may read this one".
     *
     * There is deliberately **no per-value form**: `has("read", "/etc/passwd")`
     * throws a `TypeError`. Which paths, hosts or programs are allowed is set
     * by the deployment (`--allow-<name>=<list>`), not asked about by the
     * application — and the honest answer for one value is to perform the
     * operation and catch the denial (`ERR_PERMISSION_DENIED`), since the
     * runtime resolves a path before judging it and any advance answer could be
     * stale by the time the call happens.
     */
    has(name: PermissionName): boolean;
  };

  const process: {
    env: typeof env;
    args: typeof args;
    platform: typeof platform;
    arch: typeof arch;
    cwd: typeof cwd;
    exit: typeof exit;
    unmask: typeof unmask;
    Secret: typeof Secret;
    signals: typeof signals;
    onSignal: typeof onSignal;
    offSignal: typeof offSignal;
    permissions: typeof permissions;
  };
  export default process;
}
