declare module "runtime:diagnostics" {
  /** What produced a span. */
  export type SpanKind = "op" | "timer" | "user" | "request" | "tick";

  /** How a span finished. */
  export type SpanStatus = "ok" | "error" | "cancelled";

  /** A value an attribute may carry. */
  export type AttributeValue = string | number | boolean;

  /** Narrows which records a subscription receives. Applied host-side. */
  export interface Filter {
    /** Only these kinds. Omitted means every kind. */
    kinds?: SpanKind[];
    /** Milliseconds. Records shorter than this are discarded host-side. */
    minDuration?: number;
    /**
     * `0..1`. Applied host-side and **per trace**, not per record, so a trace is
     * kept whole or dropped whole — a half-sampled trace is worse than no trace.
     * A record belonging to no trace is always kept.
     */
    sample?: number;
    /** Records held before dropping. Default 4096. Per subscription. */
    bufferSize?: number;
  }

  /**
   * One finished span.
   *
   * Field names follow OpenTelemetry so an exporter attaches with no translation
   * layer.
   *
   * The three timestamps mean the same thing whatever produced the record:
   * `scheduledAt` is when the work became runnable, `startedAt` when it began,
   * `endedAt` when it finished — so `startedAt - scheduledAt` is always queue
   * delay. For a `timer` that is lag past its **deadline**, not the delay it
   * asked for. For an `op` the two are equal, because there is no boundary
   * between them the host can observe.
   */
  export interface SpanRecord {
    /** Unique within the agent. User spans share this id space. */
    id: number;
    /**
     * The span this one ran **inside**, or `null` for a root span — a span id, in
     * the same space as `id`, so a set of records forms a tree.
     *
     * Not the parent *task*: two sibling ops inside one request share a parent
     * span while having different parent tasks. Task lineage is
     * `currentTask().parentId` in `runtime:context`.
     */
    parentId: number | null;
    /** The trace this belongs to, or `null`. */
    traceId: string | null;
    /** The op's name, the timer's function, or a user span's name. */
    name: string;
    kind: SpanKind;
    source: "runtime" | "user";
    /** Monotonic milliseconds, fractional — the same clock as `performance.now()`. */
    scheduledAt: number;
    startedAt: number;
    endedAt: number;
    status: SpanStatus;
    /**
     * Why it failed, or `null`.
     *
     * Payload, not status: a failure message routinely names the thing that
     * failed, so it is populated only with `diagnostics-detail` — exactly like
     * `attributes`. A failed span still reports `status: "error"` without it.
     */
    statusMessage: string | null;
    /** Populated only with `diagnostics-detail`; otherwise an empty object. */
    attributes: Readonly<{ [key: string]: AttributeValue }>;
    /**
     * Which turn of the loop this landed in. Read beside a `tick` record it
     * separates "this was slow" from "this waited behind something else".
     */
    tick: number;
  }

  /** One delivery to one subscriber. */
  export interface Batch {
    records: readonly SpanRecord[];
    /** Records lost to buffer overflow since the previous batch. */
    dropped: number;
  }

  export interface Subscription {
    /**
     * Stops delivery. The records buffered but not yet delivered are handed to
     * the callback first, synchronously — delivery is per loop turn, so closing
     * inside one would otherwise discard everything recorded in it.
     *
     * Idempotent.
     */
    close(): void;
  }

  /**
   * Receives batches of records matching `filter`. Delivery is push, once per
   * loop turn per subscription — never once per record.
   *
   * Needs `diagnostics`. With only that, `attributes` come back empty; with
   * `diagnostics-detail` they are populated.
   */
  export function subscribe(filter: Filter, onBatch: (batch: Batch) => void): Subscription;

  /** One kind of host handle this agent owns. */
  export interface HandleGroup {
    /** What these ids name: `"socket"`, `"HTTP server"`, `"database"`, … */
    kind: string;
    count: number;
    /** The host-side ids. Never the resource itself. */
    ids: readonly number[];
  }

  /**
   * The host handles this agent **owns**.
   *
   * Not the same as "handles that are live": a socket, a child process, a file
   * descriptor, a database connection and a worker are released when they end,
   * but a listener, an HTTP server, a WebSocket and an in-flight request are
   * deliberately kept for the lifetime of the agent, so those kinds over-report.
   *
   * An id and a kind — never the resource object, which is what made Node's
   * `async_hooks` impossible to fix.
   *
   * Needs `diagnostics`.
   */
  export function inventory(): { handles: readonly HandleGroup[] };

  /** A summary of a set of samples. */
  export interface Histogram {
    /** Samples taken. Quantiles are over the most recent 1024. */
    count: number;
    min: number;
    max: number;
    mean: number;
    p50: number;
    p99: number;
  }

  /** What {@link metrics} reports. Pull-only. */
  export interface Metrics {
    /** The turn currently running. */
    tick: number;
    /** Turns so far. */
    ticks: number;
    /** How long each turn took. A long turn blocks everything else in it. */
    tickDurationMs: Histogram;
    /**
     * How long the loop was not running between turns. Read beside
     * `tickDurationMs`: a large gap with short turns is an idle process, a large
     * gap with long turns is a loop that cannot keep up.
     */
    loopLagMs: Histogram;
  }

  /** The loop's own numbers. Needs `diagnostics`. */
  export function metrics(): Metrics;

  /** A span the program opened. Ending it twice records it once. */
  export interface Span {
    /** Ends it with `status: "ok"`. */
    end(): void;
    /** Ends it with `status: "error"`. */
    fail(): void;
    /** Ends it with `status: "cancelled"`. */
    cancel(): void;
  }

  /**
   * Opens a span the program ends itself. It shares the id space and the
   * timeline with the runtime's own spans; `source: "user"` tells them apart.
   *
   * The handle form records its parent and **nests nothing**. For the work
   * inside a span to become its children, use the callback form below — the
   * same split OpenTelemetry makes between `startSpan` and `startActiveSpan`.
   *
   * `attributes` are recorded only under `diagnostics-detail`.
   *
   * Needs `diagnostics`.
   */
  export function span(
    name: string,
    options?: { attributes?: Readonly<{ [key: string]: AttributeValue }> },
  ): Span;

  /**
   * Runs `fn` with the span **active**, so everything `fn` does — and everything
   * it schedules — nests under it. Ends when `fn` returns, or when its promise
   * settles; `status` follows whether it threw.
   *
   * Returns exactly what `fn` returned.
   */
  export function span<R>(
    name: string,
    options: { attributes?: Readonly<{ [key: string]: AttributeValue }> } | undefined,
    fn: () => R,
  ): R;

  const _default: {
    subscribe: typeof subscribe;
    inventory: typeof inventory;
    metrics: typeof metrics;
    span: typeof span;
  };
  export default _default;
}
