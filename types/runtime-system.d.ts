declare module "runtime:system" {
  /** A signal name this runtime can send to a child process. */
  export type SignalName =
    | "SIGINT"
    | "SIGTERM"
    | "SIGHUP"
    | "SIGQUIT"
    | "SIGKILL"
    | "SIGUSR1"
    | "SIGUSR2"
    | "SIGBREAK";

  /** How one of the child's standard streams is connected. */
  export type StdioMode = "piped" | "inherit" | "null";

  /** Anything that can be written to a child's standard input. */
  export type CommandInput =
    | string
    | Uint8Array
    | ArrayBuffer
    | ArrayBufferView
    | Blob
    | Response
    | ReadableStream<Uint8Array>;

  /** How a child process ended. */
  export interface CommandStatus {
    /** Whether the child exited with status 0. */
    readonly success: boolean;
    /** The exit status, or `null` when a signal ended the process. */
    readonly code: number | null;
    /** The name of the signal that ended the process, if one did. */
    readonly signal: string | null;
  }

  /** The result of {@link Command.output}. */
  export interface CommandOutput extends CommandStatus {
    /** Everything the child wrote to stdout (empty unless it was piped). */
    readonly stdout: Uint8Array;
    /** Everything the child wrote to stderr (empty unless it was piped). */
    readonly stderr: Uint8Array;
  }

  export interface CommandOptions {
    /** Arguments, passed to the child verbatim — no quoting or escaping needed. */
    args?: (string | number | boolean)[];
    /** The child's working directory. Defaults to the parent's. */
    cwd?: string | URL;
    /**
     * The child's environment. A `Secret` from `runtime:process` is unwrapped
     * for the child (it would otherwise arrive as the literal `"[redacted]"`);
     * `undefined` removes an inherited key.
     */
    env?: Record<string, string | { toString(): string } | undefined>;
    /**
     * Start from the host environment instead of an empty one. Requires the
     * `Env` capability in addition to `Run`.
     */
    inheritEnv?: boolean;
    /**
     * How to connect stdin, or a body to write to it (after which stdin is
     * closed). Defaults to `"null"`.
     */
    stdin?: StdioMode | CommandInput;
    /** How to connect stdout. Defaults to `"piped"`. */
    stdout?: StdioMode;
    /** How to connect stderr. Defaults to `"piped"`. */
    stderr?: StdioMode;
    /** Aborting kills the child; `output()` rejects with the abort reason. */
    signal?: AbortSignal;
    /** Kill the child after this many milliseconds. */
    timeout?: number;
    /** The signal used by `kill()`, a timeout, or an abort. Defaults to `"SIGTERM"`. */
    killSignal?: SignalName;
    /**
     * `output()` only: past this many bytes the child is killed and the call
     * throws with `code === "ERR_MAX_BUFFER"`. Defaults to 16 MiB.
     */
    maxBuffer?: number;
  }

  /**
   * A running child process.
   *
   * The streams are created on first use, and reading {@link status} is what
   * keeps the program alive until the child exits — so a child nobody waits on
   * holds nothing open, and is killed rather than orphaned when the runtime
   * goes away.
   */
  export class ChildProcess {
    private constructor();
    /** The OS process id. */
    readonly pid: number;
    /** `null` unless stdin was `"piped"`. Closing it is the child's EOF. */
    readonly stdin: WritableStream<Uint8Array | string> | null;
    /** `null` unless stdout was `"piped"`. */
    readonly stdout: ReadableStream<Uint8Array> | null;
    /** `null` unless stderr was `"piped"`. */
    readonly stderr: ReadableStream<Uint8Array> | null;
    /** Resolves once the child has exited. */
    readonly status: Promise<CommandStatus>;
    /**
     * Sends a signal (default `killSignal`). Signalling a child that has
     * already exited is a no-op. Only this child is signalled — grandchildren
     * can outlive a kill.
     */
    kill(signal?: SignalName): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
  }

  /**
   * A command to run. There is no shell: `program` plus `args` become an argv,
   * so nothing is word-split, glob-expanded, or re-parsed.
   *
   * `program` is a path (absolute, or relative to `cwd`) or a bare name looked
   * up on the **host** `PATH` — the `env` you pass describes the child's
   * environment, not where the runtime looks for executables.
   *
   * Requires the `Run` capability.
   */
  export class Command {
    constructor(program: string, options?: CommandOptions);
    /** Runs to completion, collecting stdout and stderr. */
    output(): Promise<CommandOutput>;
    /** Starts the child. Rejects if the program cannot be found or started. */
    spawn(): Promise<ChildProcess>;
  }

  const system: { Command: typeof Command; ChildProcess: typeof ChildProcess };
  export default system;
}
