declare module "runtime:context" {
  /** Options for {@link createContext}. */
  export interface ContextOptions<T> {
    /**
     * Diagnostic label. **Not an identity key** — identity is the object
     * itself, so two contexts named `"user"` are still two contexts and cannot
     * collide.
     */
    name?: string;
    /** Returned by {@link Context.get} outside any {@link Context.run} scope. Defaults to `undefined`. */
    defaultValue?: T;
  }

  /**
   * A value that follows the work rather than the call stack.
   *
   * Create one per concern and keep it module-private; nothing enumerates the
   * contexts in flight, so a library's context is reachable only by code that
   * can name the object.
   */
  export interface Context<T> {
    /** The label passed to {@link createContext}, if any. */
    readonly name: string | undefined;
    /**
     * The value current in this scope, or the configured `defaultValue`
     * outside any {@link run}.
     */
    get(): T;
    /**
     * Runs `fn` with `value` current, for `fn` and for everything `fn`
     * schedules — `await`, timers, and op callbacks alike.
     *
     * Returns exactly what `fn` returns, promise included. For an `async` `fn`
     * this returns as soon as it first yields: the mapping is reinstalled on
     * entry to each continuation rather than held until the promise settles,
     * which is what lets two requests be in flight without one's scope
     * outliving the other.
     *
     * The scope is copy-on-write, so a write here is invisible to a concurrent
     * sibling and to the caller.
     */
    run<R, A extends unknown[]>(value: T, fn: (...args: A) => R, ...args: A): R;
  }

  /** Creates a context. Ungated, like everything else in this module. */
  export function createContext<T>(options?: ContextOptions<T>): Context<T>;

  /**
   * Captures the current mapping. The returned function runs `fn` under it.
   *
   * The replacement for Node's removed `enterWith`: a scope with an end.
   */
  export function snapshot(): <R, A extends unknown[]>(fn: (...args: A) => R, ...args: A) => R;

  /**
   * Pins `fn` to the mapping current at `bind()` time, forwarding `this`.
   *
   * Needed wherever propagation deliberately stops — above all an
   * `EventTarget` listener, which otherwise runs in the mapping of whoever
   * called `dispatchEvent`.
   */
  export function bind<F extends (...args: never[]) => unknown>(fn: F): F;

  /**
   * Runs `fn` with an explicit trace id — for a queue consumer adopting a trace
   * that started upstream.
   *
   * `traceId` must be a W3C trace-id: 32 hex characters, not all zero.
   * Uppercase is normalized; anything else is a `TypeError`.
   */
  export function withTrace<R>(traceId: string, fn: () => R): R;

  /** What {@link currentTask} reports. A copy; writing to it changes nothing. */
  export interface TaskInfo {
    /** This task's id. A task is the root, a timer firing or an inbound
     * request; an `await` stays in the task that reached it. */
    id: number;
    /** The id of the task that started this one, or `null` at the root. */
    parentId: number | null;
    /**
     * The W3C trace id this task runs under: minted per inbound request by
     * `runtime:http`, set explicitly by {@link withTrace}, and otherwise minted
     * once per agent on first read. Read-only — {@link withTrace} is the only
     * override.
     */
    traceId: string;
    /** What established this scope: `"main"`, `"http-request"`, `"worker"`. */
    kind: string;
  }

  /** A cheap read of the executing task. Ungated: it reveals no payload. */
  export function currentTask(): TaskInfo;

  const _default: {
    createContext: typeof createContext;
    snapshot: typeof snapshot;
    bind: typeof bind;
    withTrace: typeof withTrace;
    currentTask: typeof currentTask;
  };
  export default _default;
}
