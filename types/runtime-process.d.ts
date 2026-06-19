declare module "runtime:process" {
  /**
   * An opaque holder for a secret env value. Env entries whose key ends in
   * `_SECRET(S)` or `_PASSWORD(S)` (case-insensitive) are exposed as a `Secret`
   * instead of a raw string, so they render as `"[redacted]"` everywhere they
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
   * Values for secret-bearing keys (`*_SECRET(S)`, `*_PASSWORD(S)`) are
   * {@link Secret} wrappers; pass them through {@link unmask} to read the value.
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

  const process: {
    env: typeof env;
    args: typeof args;
    platform: typeof platform;
    arch: typeof arch;
    cwd: typeof cwd;
    exit: typeof exit;
    unmask: typeof unmask;
    Secret: typeof Secret;
  };
  export default process;
}
