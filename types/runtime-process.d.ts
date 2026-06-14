declare module "runtime:process" {
  /**
   * Environment variables as a mutable in-process object, seeded from a host
   * snapshot taken when the module is evaluated. Reads, writes, and deletes work
   * in-process; they do not propagate to the host process or to child processes.
   */
  export const env: Record<string, string>;

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
  };
  export default process;
}
