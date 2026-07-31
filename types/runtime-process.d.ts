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
  };
  export default process;
}
