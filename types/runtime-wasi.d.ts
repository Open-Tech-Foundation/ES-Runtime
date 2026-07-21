declare module "runtime:wasi" {
  /** Options for {@link WASI}. */
  export interface WASIOptions {
    /**
     * `argv` for the guest, including `argv[0]`. Defaults to `[]`.
     *
     * Nothing ambient is read: this is the only source of the guest's
     * arguments.
     */
    args?: string[];

    /**
     * Environment pairs for the guest. Defaults to `{}`.
     *
     * The host's real environment is **never** inherited — forward it
     * explicitly (via the `Env`-gated `runtime:process`) if that is what you
     * want.
     */
    env?: Record<string, string>;

    /** The WASI snapshot to implement. Only `"preview1"` is supported. */
    version?: "preview1";
  }

  /**
   * A WASI preview 1 (`wasi_snapshot_preview1`) instance.
   *
   * Build one, hand {@link WASI.getImportObject} to `WebAssembly.instantiate`,
   * then {@link WASI.start} the resulting instance.
   *
   * Arguments, environment, clocks, randomness, stdio and process exit are
   * implemented. Filesystem calls are present but report `ENOTCAPABLE` (76):
   * preopens are not wired yet.
   */
  export class WASI {
    constructor(options?: WASIOptions);

    /** The `wasi_snapshot_preview1` import object to instantiate with. */
    getImportObject(): Record<string, WebAssembly.ModuleImports>;

    /**
     * Runs a command module: binds its exported memory, calls `_start`, and
     * returns the exit status — `0` if `_start` returned normally, otherwise the
     * code passed to `proc_exit`. A genuine fault still throws.
     */
    start(instance: WebAssembly.Instance): number;

    /**
     * Runs a reactor module: binds its exported memory and calls `_initialize`
     * if present, leaving the instance live for the caller to drive.
     */
    initialize(instance: WebAssembly.Instance): void;
  }
}
