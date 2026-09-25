declare module "runtime:workers" {
  import type { WebSocketConnection } from "runtime:websocket";

  /**
   * Settings for every durable worker in this process.
   *
   * Optional: with no call at all the defaults apply. It must come before the
   * first worker is materialized, since these decide where state lives.
   */
  export interface DurableConfig {
    /** Where state lives. A relative path is relative to the working directory,
     * not to the entry file. Default `"./.durable"`. */
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
    /**
     * How many shards (`Worker`s) run the classes' code, while their state
     * stays on this agent. `"auto"` sizes it from the machine. Default `0`: the
     * code runs on the agent that addressed the worker. Needs `module`.
     */
    shards?: number | "auto";
    /**
     * The module the durable-worker classes are exported from, as an absolute
     * URL — every shard imports it. `new URL("./classes.js", import.meta.url)`.
     */
    module?: string | URL | null;
    /** What a shard is granted, beside the `imports` it always has. Default `[]`. */
    permissions?: string[];
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

  /** What a class declares in `static schema`. */
  export interface DurableSchema {
    collections?: Record<string, { index?: string[]; unique?: string[] }>;
  }

  /** A value a declared field may hold: what a column can be ordered by. */
  export type DurableField = string | number | boolean | Date | bigint | null | undefined;

  /** How one field is compared. A bare value means equality. */
  export type DurableTest =
    | DurableField
    | {
        eq?: DurableField;
        ne?: DurableField;
        gt?: DurableField;
        gte?: DurableField;
        lt?: DurableField;
        lte?: DurableField;
        in?: DurableField[];
      };

  /** Fields to match. Every name must be one the class declared, unless the
   * query was made with `{ scan: true }`. */
  export type DurableWhere = Record<string, DurableTest>;

  /** A query. Nothing runs until it is iterated, `toArray`-ed or counted. */
  export interface DurableQuery<T> extends AsyncIterable<T> {
    sort(order: Record<string, "asc" | "desc">): DurableQuery<T>;
    limit(n: number): DurableQuery<T>;
    offset(n: number): DurableQuery<T>;
    toArray(): Promise<T[]>;
    first(): Promise<T | null>;
    count(): Promise<number>;
  }

  /** Documents in a table of their own: queried rather than held. */
  export interface DurableCollection<T = Record<string, unknown>> {
    readonly name: string;
    /** Stores it, returning its id — `doc.id`, or a fresh UUID. */
    insert(doc: T): Promise<string>;
    /** Several in one statement. */
    insertMany(docs: Iterable<T>): Promise<string[]>;
    get(id: string): Promise<T | undefined>;
    /** Merges an object, or applies a function. Resolves to what is now stored. */
    update(id: string, patch: Partial<T> | ((doc: T) => T | Promise<T>)): Promise<T | undefined>;
    delete(id: string): Promise<boolean>;
    /** Removes everything `where` selects; resolves to how many that was. */
    deleteWhere(where: DurableWhere): Promise<number>;
    find(where?: DurableWhere, options?: { scan?: boolean }): DurableQuery<T>;
    count(where?: DurableWhere, options?: { scan?: boolean }): Promise<number>;
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
    /** `worker` is the one it happened to — `null` for a failure of the
     * scheduler itself. `gaveUp` is true when the alarm is gone for good,
     * false when the next sweep will try again. */
    onError?: (
      error: unknown,
      context: string,
      worker: { name: string; id: string; gaveUp: boolean } | null,
    ) => void;
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
    /** A collection the class declared in `static schema`. */
    collection<T = Record<string, unknown>>(name: string): DurableCollection<T>;
    /** Runs `work` in one transaction over the keys and the collections alike. */
    transaction<T>(work: () => T | Promise<T>): Promise<T>;
  }

  /** What a worker knows about itself. */
  export interface DurableContext {
    readonly id: string;
    /** The class's storage name. */
    readonly name: string;
    /** Aborts when this worker is being closed. */
    readonly signal: AbortSignal;
    /**
     * Takes ownership of a WebSocket this call was handed. The runtime holds it
     * from now on, so the worker can hibernate while it stays connected, and
     * its events arrive at `webSocketMessage` / `webSocketClose` /
     * `webSocketError`. At most 10 tags, 256 characters each.
     */
    acceptWebSocket(ws: DurableSocket, tags?: string[]): void;
    /** The sockets this worker has accepted, optionally only those with `tag`. */
    getWebSockets(tag?: string): DurableSocket[];
    /** The tags `ws` was accepted with. */
    getTags(ws: DurableSocket): string[];
    /**
     * Answers a message that is exactly `request` with `response` without
     * waking the worker — for an application heartbeat. `null` removes it.
     * Held in memory: set it in `start()`.
     */
    setWebSocketAutoResponse(pair: { request: string; response: string } | null): void;
    getWebSocketAutoResponse(): { request: string; response: string } | null;
    /** When `ws` was last answered by the auto-response, or `null`. */
    getWebSocketAutoResponseTimestamp(ws: DurableSocket): Date | null;
  }

  /**
   * A worker's handle on a WebSocket it was handed or owns. `send` waits until
   * the worker's writes so far are committed, and sends on one socket keep
   * their order.
   */
  export interface DurableSocket {
    readonly protocol: string;
    send(data: string | ArrayBuffer | ArrayBufferView): void;
    close(code?: number, reason?: string): void;
    /** Keeps a structured-clonable value with the socket across hibernation.
     * At most 16 KiB serialized. */
    serializeAttachment(value: unknown): void;
    deserializeAttachment<T = unknown>(): T | null;
  }

  /** A socket as a caller passes it: the connection itself, or a handle a
   * worker already holds. */
  type SocketArgument<T> = T extends DurableSocket ? DurableSocket | WebSocketConnection | WebSocket : T;

  /**
   * A reference to a durable worker: its methods, returning promises. Nothing
   * is opened until one of them is called.
   */
  export type DurableRef<T> = { readonly id: string } & {
    [K in keyof T as T[K] extends (...args: never[]) => unknown
      ? K extends
          | "start"
          | "stop"
          | "alarm"
          | "webSocketMessage"
          | "webSocketClose"
          | "webSocketError"
          | "state"
          | "ctx"
          | "id"
        ? never
        : K
      : never]: T[K] extends (...args: infer A) => infer R
      ? (...args: { [I in keyof A]: SocketArgument<A[I]> }) => Promise<Awaited<R>>
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

    /** The collections this worker keeps, and which of their fields are
     * queryable. Applied to a worker's own file the first time it is opened
     * after a change. */
    static schema?: DurableSchema;

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
    /** A message on a socket this worker accepted. Wakes it if it is hibernating. */
    webSocketMessage?(ws: DurableSocket, message: string | ArrayBuffer): void | Promise<void>;
    /** A socket this worker accepted closed. */
    webSocketClose?(ws: DurableSocket, code: number, reason: string, wasClean: boolean): void | Promise<void>;
    /** A socket this worker accepted failed. */
    webSocketError?(ws: DurableSocket, error: unknown): void | Promise<void>;
  }

  /** Stable `code` values on a {@link DurableError}. */
  export const DurableErrorCode: Readonly<{
    Busy: "ERR_DURABLE_BUSY";
    Locked: "ERR_DURABLE_LOCKED";
    StateTooLarge: "ERR_DURABLE_STATE_TOO_LARGE";
    StateFormat: "ERR_DURABLE_STATE_FORMAT";
    IdCollision: "ERR_DURABLE_ID_COLLISION";
    Shutdown: "ERR_DURABLE_SHUTDOWN";
    ShardLost: "ERR_DURABLE_SHARD_LOST";
    Cycle: "ERR_DURABLE_CYCLE";
    Configured: "ERR_DURABLE_CONFIGURED";
  }>;

  export class DurableError extends Error {
    readonly name: "DurableError";
    readonly code: string;
  }

  export function configure(
    options?: DurableConfig,
  ): Required<Omit<DurableConfig, "shards" | "module">> & { shards: number; module: string | null };

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
