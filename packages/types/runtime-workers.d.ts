declare module "runtime:workers" {
  /**
   * Settings for every durable worker in this process.
   *
   * Optional: with no call at all the defaults apply. It must come before the
   * first worker is materialized, since these decide where state lives.
   */
  export interface DurableConfig {
    /** Where state lives, relative to the working directory. Default `"./.durable"`. */
    dir?: string;
    /** How long a worker may sit idle before it is closed. Default `30_000` ms. */
    evictAfter?: number;
    /** How many workers may be open at once. Default `128`. */
    maxLive?: number;
    /** How many calls may wait on one worker before further ones are refused. Default `1024`. */
    mailbox?: number;
    /** The ceiling on one worker's whole key/value state, in bytes. Default 1 MiB. */
    stateLimit?: number;
    /** The ceiling on a single stored value, in bytes. Default 128 KiB. */
    valueLimit?: number;
    /** How many times a failing `alarm()` is retried. Default `5`. */
    alarmRetries?: number;
    /** The longest the alarm scheduler sleeps between looks. Default `60_000` ms. */
    alarmPoll?: number;
  }

  /** Narrows what {@link DurableState.keys} and {@link DurableState.list} return. */
  export interface DurableKeyRange {
    prefix?: string;
    /** Inclusive lower bound. */
    start?: string;
    /** Exclusive upper bound. */
    end?: string;
    limit?: number;
    reverse?: boolean;
  }

  /** One row of {@link DurableWorker.list}. */
  export interface DurableWorkerInfo {
    id: string;
    createdAt: Date;
    lastActive: Date;
    /** How many bytes of state it holds. */
    bytes: number;
    /** Whether it is open in this process right now. */
    live: boolean;
  }

  /** When this worker's `alarm()` should next run. */
  export interface DurableAlarm {
    /** The time set, or `null`. Synchronous, like the rest of the state. */
    get(): Date | null;
    /** Sets it; resolves when it is durable. A time in the past runs at once. */
    set(when: Date | number): Promise<void>;
    /** Unsets it. */
    delete(): Promise<void>;
  }

  /** What {@link startAlarms} returns. */
  export interface AlarmScheduler {
    /** Stops servicing alarms; resolves when the sweep in flight has finished. */
    stop(): Promise<void>;
    /** Whether it is still running. */
    readonly running: boolean;
  }

  export interface AlarmOptions {
    /**
     * The durable worker classes this process runs alarms for. Required:
     * anything scheduled for a class not listed is left for the process that
     * does list it.
     */
    classes: Array<typeof DurableWorker>;
    /** Hears about an alarm that failed for the last time. Defaults to `console.error`. */
    onError?: (error: unknown, context: string) => void;
    /** How many due workers one sweep wakes. Default `32`. */
    batch?: number;
  }

  /**
   * A durable worker's key/value state: resident in memory, so reads are
   * synchronous, and written behind the call that changed it.
   *
   * Anything `structuredClone` can carry can be stored — `Date`, `Map`, `Set`,
   * typed arrays, `BigInt`, cycles — not only what JSON survives.
   */
  export interface DurableState {
    /** The value stored under `key`, or `undefined`. */
    get<T = unknown>(key: string): T | undefined;
    has(key: string): boolean;
    /** Stores `value`; the promise resolves once it is durable. */
    set(key: string, value: unknown): Promise<void>;
    /** Several keys in one commit. */
    setMany(entries: Record<string, unknown> | Map<string, unknown>): Promise<void>;
    delete(key: string): Promise<void>;
    deleteMany(keys: Iterable<string>): Promise<void>;
    clear(): Promise<void>;
    getMany<T = unknown>(keys: Iterable<string>): Map<string, T | undefined>;
    /** The keys, sorted. */
    keys(range?: DurableKeyRange): string[];
    /** `[key, value]` pairs, sorted by key. */
    list<T = unknown>(range?: DurableKeyRange): Array<[string, T]>;
    /** How many keys are stored. */
    readonly size: number;
    /** How many bytes they take — what `stateLimit` is measured against. */
    readonly bytes: number;
    /** Waits for every write made so far to be durable. */
    sync(): Promise<void>;
    /** When this worker's `alarm()` should next run. */
    readonly alarm: DurableAlarm;
  }

  /** What a worker knows about itself. */
  export interface DurableContext {
    readonly id: string;
    /** The class's storage name. */
    readonly name: string;
    /** Aborts when this worker is being closed. */
    readonly signal: AbortSignal;
  }

  /**
   * A reference to a durable worker: its methods, returning promises. Nothing
   * is opened until one of them is called.
   */
  export type DurableRef<T> = { readonly id: string } & {
    [K in keyof T as T[K] extends (...args: never[]) => unknown
      ? K extends "start" | "stop" | "alarm" | "state" | "ctx" | "id"
        ? never
        : K
      : never]: T[K] extends (...args: infer A) => infer R
      ? (...args: A) => Promise<Awaited<R>>
      : never;
  };

  /**
   * The base class of every durable worker.
   *
   * Extend it, add methods, and address one by id. The runtime materializes it
   * on first use, runs one call at a time against it, and closes it when it has
   * been idle — its state outliving all of that.
   *
   *     export class Counter extends DurableWorker {
   *       async add(n: number) {
   *         const total = (this.state.get<number>("total") ?? 0) + n;
   *         this.state.set("total", total);
   *         return total;
   *       }
   *     }
   *
   *     await Counter.get("hits").add(1);
   */
  export class DurableWorker {
    /** The storage name, if the class name is not the right one. */
    static durableName?: string;

    /** A reference to the worker of this class with `id`. */
    static get<T extends DurableWorker>(this: new () => T, id: string): DurableRef<T>;

    /** Closes the worker if it is open, then deletes its state for good. */
    static delete(id: string): Promise<boolean>;

    /** The ids of this class's workers, most recently active first. */
    static list(options?: { limit?: number; after?: number }): Promise<DurableWorkerInfo[]>;

    /** This worker's key/value state. */
    readonly state: DurableState;
    /** The id it was addressed by. */
    readonly id: string;
    readonly ctx: DurableContext;

    /** Runs after the state is loaded, before the first call. */
    start?(): void | Promise<void>;
    /** Runs before the worker is closed — `"idle"`, `"shutdown"` or `"deleted"`. */
    stop?(reason: string): void | Promise<void>;
    /** Runs when the alarm set on this worker comes due. */
    alarm?(): void | Promise<void>;
  }

  /** Stable `code` values on a {@link DurableError}. */
  export const DurableErrorCode: Readonly<{
    Busy: "ERR_DURABLE_BUSY";
    Locked: "ERR_DURABLE_LOCKED";
    StateTooLarge: "ERR_DURABLE_STATE_TOO_LARGE";
    StateFormat: "ERR_DURABLE_STATE_FORMAT";
    IdCollision: "ERR_DURABLE_ID_COLLISION";
    Shutdown: "ERR_DURABLE_SHUTDOWN";
    Configured: "ERR_DURABLE_CONFIGURED";
  }>;

  export class DurableError extends Error {
    readonly name: "DurableError";
    readonly code: string;
  }

  export function configure(options?: DurableConfig): Required<DurableConfig>;

  /**
   * Starts servicing alarms: due workers are woken and their `alarm()` runs.
   * While it is running the process stays alive.
   */
  export function startAlarms(options: AlarmOptions): AlarmScheduler;

  /**
   * Closes every open worker — flushing what they wrote, running their `stop()`
   * — and releases this process's hold on the directory.
   */
  export function shutdown(): Promise<void>;
}
