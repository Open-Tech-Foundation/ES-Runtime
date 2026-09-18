// A type test for `runtime:diagnostics`.
//
// Same reasoning as the others: these declarations describe a surface they do
// not implement, so nothing else catches them being wrong. `@ts-expect-error`
// fails the build when the error it names stops happening, so a declaration that
// quietly widened to `any` breaks this file rather than passing it.

import type {
  AttributeValue,
  Batch,
  Filter,
  HandleGroup,
  Histogram,
  Metrics,
  Span,
  SpanKind,
  SpanRecord,
  SpanStatus,
  Subscription,
} from "runtime:diagnostics";
import { inventory, metrics, span, subscribe } from "runtime:diagnostics";

// --- subscribe ----------------------------------------------------------------

const sub: Subscription = subscribe({}, (batch: Batch) => {
  const dropped: number = batch.dropped;
  for (const record of batch.records) {
    const id: number = record.id;
    const parent: number | null = record.parentId;
    const trace: string | null = record.traceId;
    const kind: SpanKind = record.kind;
    const status: SpanStatus = record.status;
    const source: "runtime" | "user" = record.source;
    const delay: number = record.startedAt - record.scheduledAt;
    const turn: number = record.tick;
    void [id, parent, trace, kind, status, source, delay, turn, dropped];
  }
});
sub.close();

const filter: Filter = { kinds: ["op", "timer"], minDuration: 5, sample: 0.1, bufferSize: 128 };
subscribe(filter, () => {});

// @ts-expect-error — "microtask" is not a kind this runtime records.
subscribe({ kinds: ["microtask"] }, () => {});

// @ts-expect-error — onBatch is required.
subscribe({});

// @ts-expect-error — the batch is read-only; a consumer is not editing a reading.
subscribe({}, (batch) => batch.records.push({} as SpanRecord));

// @ts-expect-error — `worker` was dropped: this runtime has no scheduler tiers.
subscribe({}, (batch) => batch.records[0].worker);

// --- records ------------------------------------------------------------------

declare const record: SpanRecord;
const attr: AttributeValue | undefined = record.attributes.path;

// @ts-expect-error — attributes are read-only.
record.attributes.path = "x";

// @ts-expect-error — and they carry no structures, only label-shaped values.
const nested: { deep: string } = record.attributes.thing;

// --- inventory ----------------------------------------------------------------

const handles: readonly HandleGroup[] = inventory().handles;
const kind: string = handles[0].kind;
const ids: readonly number[] = handles[0].ids;

// @ts-expect-error — an inventory hands back ids, never the resource.
const resource: object = handles[0].resource;

// --- metrics ------------------------------------------------------------------

const m: Metrics = metrics();
const hist: Histogram = m.tickDurationMs;
const p99: number = hist.p99;
const lag: number = m.loopLagMs.mean;

// @ts-expect-error — no pool saturation: there is no pool to saturate.
const saturation: number = m.poolSaturation;

// The process-wide half, which `runtime:process` deliberately does not report.
const rss: number = m.process.rss;
const processCpu: number = m.process.cpu;
const processUptime: number = m.process.uptime;

// @ts-expect-error — per-agent figures stay in `runtime:process`; this object is
// the process, so it has no heap of its own.
const heap: number = m.process.heapUsed;

// GC sits beside the loop numbers, because a pause is lag.
const collections: number = m.gc.count;
const pauseP99: number = m.gc.pauseMs.p99;

// @ts-expect-error — no per-space breakdown: that is a V8 internal, and
// freezing one into public API is the mistake this module was built to avoid.
const spaces: object[] = m.gc.spaces;

// --- span ---------------------------------------------------------------------

const s: Span = span("checkout");
s.end();
s.fail();
s.cancel();
span("query", { attributes: { table: "users", rows: 3, cached: false } });

// @ts-expect-error — a name is required and is a string.
span(42);

// @ts-expect-error — attributes are label-shaped, not arbitrary objects.
span("x", { attributes: { nested: { a: 1 } } });

// @ts-expect-error — `origin` was dropped: a source position needs a stack
// frame per span, which this module refuses to capture.
const origin: number = record.origin;

export {
  attr,
  collections,
  handles,
  heap,
  hist,
  ids,
  kind,
  lag,
  m,
  nested,
  origin,
  p99,
  pauseP99,
  processCpu,
  processUptime,
  resource,
  rss,
  s,
  saturation,
  spaces,
  sub,
};

// --- the two span forms -------------------------------------------------------

// The handle form: a measurement that nests nothing.
const handle: Span = span("checkout");
handle.end();

// The callback form: active for that call, and transparent to what it returns.
const scopedNumber: number = span("work", {}, () => 1);
const scopedPromise: Promise<string> = span("work", {}, async () => "x");
// Transparent to what it returns, even when that is `void`: like above, the
// `void` result is asserted through a `() => void` rather than a variable.
const scopedWithAttrs: () => void = () =>
  span("work", { attributes: { table: "users" } }, () => {});
scopedWithAttrs();

// @ts-expect-error — the callback takes no arguments; bind what it needs.
span("work", {}, (a: number) => a);

// @ts-expect-error — and `request` is the runtime's kind, not something to open.
const notAKind: SpanKind = "microtask";

export { handle, notAKind, scopedNumber, scopedPromise, scopedWithAttrs };
