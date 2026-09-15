//! The host half of `runtime:diagnostics`: a ring buffer of spans, filtered
//! here and never in JS (DECISIONS.md D89).
//!
//! The point of this module is what it does **not** do. Node's `async_hooks`
//! calls into JS for every async resource, which is why enabling it costs what
//! it costs; here an event that nothing is subscribed to, or that every
//! subscription's filter rejects, costs an integer compare and never reaches the
//! JS engine at all. [`Recorder::wants`] is that compare.
//!
//! # The three timestamps
//!
//! Every record carries the same three, with the same meaning, whatever produced
//! it:
//!
//! | | |
//! |---|---|
//! | `scheduled_at` | when the work became **runnable** |
//! | `started_at` | when it actually started running |
//! | `ended_at` | when it finished |
//!
//! So `started_at - scheduled_at` is always queue delay and never anything else.
//! For a timer, "became runnable" is its deadline — *not* when it was armed —
//! because the requested delay is not a queue, and folding it in would make the
//! field read as lag when it is mostly sleep. For an op there is no observable
//! boundary between becoming runnable and starting, so the two are equal; that
//! is reported honestly rather than papered over with a fabricated number (see
//! [`SpanKind::Op`]).
//!
//! # Loop-tick attribution
//!
//! This runtime is a **driven loop** (D4): the embedder owns `tick()`, so there
//! is an exact tick boundary no other runtime exposes. Every record carries the
//! `tick` it landed in, and a [`SpanKind::Tick`] record gives that turn's own
//! three timestamps. Together they answer the question timings alone cannot —
//! whether a span was slow or merely waited behind something else in its turn.
//! That is this module's reason to exist beyond parity.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

/// What produced a span. One bit each, so "does anything want this kind?" is a
/// mask test on the recording path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// A host op: a filesystem call, a query, a request, a socket read.
    ///
    /// `scheduled_at == started_at`. An op's future is polled eagerly at
    /// dispatch and the provider's internals are behind a trait boundary, so
    /// there is no moment the host can point at and call "started". Reporting
    /// the two as equal says "no queue is visible here"; inventing a difference
    /// would be worse than saying nothing.
    Op,
    /// A timer callback. `scheduled_at` is its deadline, so the queue delay is
    /// the lag past when it was due.
    Timer,
    /// A span the guest opened with `span()`.
    User,
    /// One inbound HTTP request, opened by `runtime:http` — the root of that
    /// request's trace, covering the handler and its streamed response.
    Request,
    /// One turn of the driven loop.
    Tick,
}

impl SpanKind {
    /// Every kind, for building a mask from a filter's names.
    pub const ALL: [SpanKind; 5] = [
        SpanKind::Op,
        SpanKind::Timer,
        SpanKind::User,
        SpanKind::Request,
        SpanKind::Tick,
    ];

    /// This kind's bit within a [`Recorder::wants`] mask.
    pub const fn bit(self) -> u8 {
        match self {
            SpanKind::Op => 1 << 0,
            SpanKind::Timer => 1 << 1,
            SpanKind::User => 1 << 2,
            SpanKind::Request => 1 << 3,
            SpanKind::Tick => 1 << 4,
        }
    }

    /// The name this kind is filtered and reported by. One vocabulary across the
    /// filter, the record and the docs.
    pub const fn name(self) -> &'static str {
        match self {
            SpanKind::Op => "op",
            SpanKind::Timer => "timer",
            SpanKind::User => "user",
            SpanKind::Request => "request",
            SpanKind::Tick => "tick",
        }
    }

    /// The kind a filter name refers to, or `None` if it names no kind.
    pub fn from_name(name: &str) -> Option<SpanKind> {
        SpanKind::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// How a span finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanStatus {
    /// Finished normally.
    Ok,
    /// Raised or rejected.
    Error,
    /// Stopped before finishing — a terminated callback.
    Cancelled,
}

impl SpanStatus {
    /// The name this status is reported by.
    pub const fn name(self) -> &'static str {
        match self {
            SpanStatus::Ok => "ok",
            SpanStatus::Error => "error",
            SpanStatus::Cancelled => "cancelled",
        }
    }
}

/// A value an attribute may carry. Deliberately not "any JS value": an
/// attribute is something an exporter writes into a label, and a structure
/// there is a structure somebody has to flatten later.
#[derive(Clone, Debug, PartialEq)]
pub enum AttrValue {
    /// A string — a path, a URL, a statement.
    Str(Rc<str>),
    /// A number.
    Num(f64),
    /// A boolean.
    Bool(bool),
}

/// One finished span.
///
/// Field names follow OpenTelemetry so an exporter attaches with no translation
/// layer, which is why none of them is shortened.
#[derive(Clone, Debug)]
pub struct SpanRecord {
    /// This span's id, unique within the agent and shared with user spans.
    pub id: u64,
    /// The span this one ran **inside**, or `None` for a root span.
    ///
    /// A span id, in the same space as `id`, so a set of records forms a tree an
    /// exporter can walk. Deliberately *not* the parent task: task lineage is
    /// `runtime:context`'s answer (`currentTask().parentId`) and is a different
    /// question — two sibling ops inside one request share a parent span while
    /// having different parent tasks.
    pub parent_id: Option<u64>,
    /// The trace this span belongs to, as `runtime:context` reports it.
    pub trace_id: Option<Rc<str>>,
    /// The op's name, the timer's function, or the name a user span was given.
    pub name: Rc<str>,
    /// What produced it.
    pub kind: SpanKind,
    /// `true` for a span the guest opened; `false` for one the runtime did.
    /// User spans share the runtime id space deliberately — one timeline.
    pub user: bool,
    /// When the work became runnable (see the module docs).
    pub scheduled_at: f64,
    /// When it started running.
    pub started_at: f64,
    /// When it finished.
    pub ended_at: f64,
    /// How it finished.
    pub status: SpanStatus,
    /// Why it failed, when it did and when somebody paid to know.
    ///
    /// Treated as payload, not as status: a failure message routinely names the
    /// thing that failed — `cannot read ./secrets.env` — so it is collected only
    /// under `diagnostics:detail`, exactly like `attributes`. Without it a failed
    /// span still says *that* it failed, which is what an error rate needs.
    pub status_message: Option<Rc<str>>,
    /// Empty unless a subscription holds `diagnostics:detail`.
    pub attributes: Vec<(Rc<str>, AttrValue)>,
    /// Which turn of the driven loop this landed in.
    pub tick: u64,
}

/// A subscription's filter, already reduced to the form the recording path
/// tests: names resolved to a mask, fractions to integers.
#[derive(Clone, Debug)]
pub struct Filter {
    /// Mask of [`SpanKind::bit`]s. `0` means every kind.
    pub kinds: u8,
    /// Records shorter than this are discarded host-side.
    pub min_duration: f64,
    /// The threshold a trace's hash must fall under to be kept, as a fraction of
    /// `u64::MAX`. `u64::MAX` keeps everything.
    pub sample: u64,
    /// Records held before dropping.
    pub buffer_size: usize,
}

impl Default for Filter {
    fn default() -> Self {
        Filter {
            kinds: 0,
            min_duration: 0.0,
            sample: u64::MAX,
            buffer_size: Recorder::DEFAULT_BUFFER,
        }
    }
}

/// Where a subscription's batches go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sink {
    /// A `subscribe()` call in the guest; delivered by a call into JS.
    Js,
    /// The runtime itself — the OTLP exporter. Drained host-side and never
    /// dispatched, so exporting telemetry needs no capability in the *program*:
    /// a deployment that exports traces does not thereby let its own code read
    /// them, or reach the collector.
    Host,
}

/// One subscriber's ring and its loss counter.
struct Subscription {
    id: u64,
    sink: Sink,
    filter: Filter,
    /// Bounded by `filter.buffer_size`; **drop newest** on overflow.
    ///
    /// Drop-oldest would evict a span's start while its end was still to come,
    /// leaving a batch of orphans; drop-newest keeps what is already there
    /// consistent and makes the loss visible as a count rather than as a hole.
    buffer: VecDeque<SpanRecord>,
    dropped: u64,
    /// Whether this subscription holds `diagnostics:detail`.
    detail: bool,
}

/// A fixed-capacity reservoir of samples, summarized on demand.
///
/// Bounded and allocation-free after the first fill: `metrics()` is pull-only,
/// so nothing drains this and an unbounded one would be a leak proportional to
/// uptime. Once full, a sample replaces the oldest — the recent shape of the
/// loop is what a reader is asking about, and an hour-old tick is not evidence
/// about the current one.
#[derive(Clone, Debug, Default)]
pub struct Histogram {
    /// Every sample ever taken, which `count` reports even though only the last
    /// [`CAPACITY`](Histogram::CAPACITY) are kept for quantiles.
    pub count: u64,
    /// The smallest sample seen.
    pub min: f64,
    /// The largest sample seen.
    pub max: f64,
    sum: f64,
    samples: Vec<f64>,
    next: usize,
}

impl Histogram {
    /// Samples kept for quantiles. 1024 doubles is 8KiB per histogram, and
    /// there are two.
    pub const CAPACITY: usize = 1024;

    fn observe(&mut self, value: f64) {
        if self.count == 0 || value < self.min {
            self.min = value;
        }
        if self.count == 0 || value > self.max {
            self.max = value;
        }
        self.count += 1;
        self.sum += value;
        if self.samples.len() < Histogram::CAPACITY {
            self.samples.push(value);
        } else {
            self.samples[self.next] = value;
            self.next = (self.next + 1) % Histogram::CAPACITY;
        }
    }

    /// The mean over every sample ever taken, not just the retained ones.
    pub fn mean(&self) -> f64 {
        if self.count == 0 {
            return 0.0;
        }
        self.sum / self.count as f64
    }

    /// The `q` quantile over the retained samples (`0.0..=1.0`).
    pub fn quantile(&self, q: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let rank = (q * (sorted.len() - 1) as f64).round() as usize;
        sorted[rank.min(sorted.len() - 1)]
    }
}

/// What `metrics()` reports about the loop itself.
#[derive(Clone, Debug, Default)]
pub struct LoopMetrics {
    /// Turns of the loop so far.
    pub ticks: u64,
    /// How long each turn took.
    pub duration: Histogram,
    /// How long the loop was **not running** between turns — the gap between one
    /// turn ending and the next beginning.
    ///
    /// Not all of it is lag: a loop with nothing to do is parked, and parking is
    /// correct. What makes it useful is reading it beside `duration` — a large
    /// gap with short turns is an idle process, and a large gap with long turns
    /// is a loop that cannot keep up.
    pub lag: Histogram,
}

/// The ring buffer, the subscriptions, and the gate in front of both.
pub struct Recorder {
    /// Union of every subscription's wanted kinds. `0` — the overwhelmingly
    /// common case — makes [`wants`](Self::wants) false for everything without
    /// touching a subscription.
    wanted: u8,
    /// Whether *any* subscription holds detail. Decides whether attributes are
    /// collected at all.
    detail: bool,
    subs: Vec<Subscription>,
    next_sub: u64,
    next_id: u64,
    tick: u64,
    /// When the turn currently running began, and when the previous one ended.
    tick_started_at: f64,
    tick_ended_at: f64,
    loop_metrics: LoopMetrics,
    /// Monotonic milliseconds, injected because the engine owns no clock — the
    /// runtime hands it one backed by the `Clock` provider, so a test with a
    /// fake clock sees fake timestamps here too (D5: no ambient authority).
    clock: Option<Rc<dyn Fn() -> f64>>,
}

impl Recorder {
    /// Records held per subscription before dropping. Per subscription rather
    /// than shared, so one slow consumer cannot evict another's records.
    pub const DEFAULT_BUFFER: usize = 4096;

    pub(crate) fn new() -> Self {
        Recorder {
            wanted: 0,
            detail: false,
            subs: Vec::new(),
            next_sub: 1,
            next_id: 1,
            tick: 0,
            tick_started_at: 0.0,
            tick_ended_at: 0.0,
            loop_metrics: LoopMetrics::default(),
            clock: None,
        }
    }

    /// The loop's own numbers, for `metrics()`.
    pub fn loop_metrics(&self) -> &LoopMetrics {
        &self.loop_metrics
    }

    /// Installs the clock. Called once, when `runtime:diagnostics` is first
    /// served; until then nothing can subscribe, so nothing needs a timestamp.
    pub(crate) fn set_clock(&mut self, clock: Rc<dyn Fn() -> f64>) {
        self.clock = Some(clock);
    }

    /// Monotonic milliseconds, or `0.0` before a clock is installed.
    pub fn now(&self) -> f64 {
        match &self.clock {
            Some(clock) => clock(),
            None => 0.0,
        }
    }

    /// **The gate.** Whether any subscription wants this kind at all.
    ///
    /// One mask test, and false for every program that never subscribed. Every
    /// recording site calls this before it does any work — before reading a
    /// clock, before allocating a name, before touching a span id.
    pub fn wants(&self, kind: SpanKind) -> bool {
        self.wanted & kind.bit() != 0
    }

    /// Whether attributes should be collected — that is, whether any
    /// subscription paid for them with `diagnostics:detail`.
    pub fn wants_detail(&self) -> bool {
        self.detail
    }

    /// Whether *anything* is being recorded, whatever the kind.
    ///
    /// Distinct from [`wants`](Self::wants) because **nesting is not filtering**.
    /// A subscriber watching only ops still wants those ops nested under the
    /// request that caused them, so the enclosing span has to be opened and made
    /// current even though its own record will be filtered away. Gating the id
    /// on `wants(Request)` instead made `{ kinds: ["op"] }` produce a flat list.
    pub fn is_recording(&self) -> bool {
        self.wanted != 0
    }

    /// The id for a span about to be recorded.
    pub fn next_span_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// The turn currently running.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Opens the next turn, recording how long the loop was not running.
    ///
    /// Unconditional, unlike the span recording: two histogram updates per turn
    /// is a fixed cost that does not scale with the program's work, and
    /// `metrics()` is pull-only — a reader asking for the first time still wants
    /// to see the shape of the loop before it asked.
    pub(crate) fn begin_tick(&mut self) -> u64 {
        self.tick += 1;
        self.tick_started_at = self.now();
        if self.tick > 1 {
            self.loop_metrics
                .lag
                .observe((self.tick_started_at - self.tick_ended_at).max(0.0));
        }
        self.loop_metrics.ticks = self.tick;
        self.tick
    }

    /// Closes the turn, recording its duration and — when anything is watching
    /// them — emitting it as a [`SpanKind::Tick`] record.
    ///
    /// The tick record is the loop-tick attribution this module exists for: read
    /// beside the `tick` field every other record carries, it says whether a
    /// span was slow or merely landed in a turn that was busy with something
    /// else. `scheduled_at` is when the previous turn ended, so the record's
    /// queue delay is the gap the loop spent not running.
    pub(crate) fn end_tick(&mut self) {
        let ended_at = self.now();
        let started_at = self.tick_started_at;
        self.loop_metrics
            .duration
            .observe((ended_at - started_at).max(0.0));
        if self.wants(SpanKind::Tick) {
            let scheduled_at = if self.tick > 1 {
                self.tick_ended_at
            } else {
                started_at
            };
            let id = self.next_span_id();
            let tick = self.tick;
            self.record(SpanRecord {
                id,
                parent_id: None,
                trace_id: None,
                name: Rc::from("tick"),
                kind: SpanKind::Tick,
                user: false,
                scheduled_at,
                started_at,
                ended_at,
                status: SpanStatus::Ok,
                status_message: None,
                attributes: Vec::new(),
                tick,
            });
        }
        self.tick_ended_at = ended_at;
    }

    /// Adds a subscription and returns its id.
    pub fn subscribe(&mut self, filter: Filter, detail: bool) -> u64 {
        self.subscribe_to(filter, detail, Sink::Js)
    }

    /// Adds a subscription delivered to `sink`.
    pub fn subscribe_to(&mut self, filter: Filter, detail: bool, sink: Sink) -> u64 {
        let id = self.next_sub;
        self.next_sub += 1;
        self.subs.push(Subscription {
            id,
            sink,
            filter,
            buffer: VecDeque::new(),
            dropped: 0,
            detail,
        });
        self.recompute();
        id
    }

    /// Removes a subscription, handing back whatever it had not yet been
    /// delivered as a final batch.
    ///
    /// Delivery happens at the end of a turn, so a subscriber that closes during
    /// one would otherwise silently lose everything recorded in it — including,
    /// commonly, the very work it subscribed to watch. Returning the remainder
    /// makes `close()` mean "give me what you have, then stop" rather than
    /// "discard the last turn".
    pub fn unsubscribe(&mut self, id: u64) -> Option<(Vec<SpanRecord>, u64)> {
        let index = self.subs.iter().position(|sub| sub.id == id)?;
        let mut sub = self.subs.remove(index);
        self.recompute();
        let records: Vec<SpanRecord> = sub.buffer.drain(..).collect();
        Some((records, sub.dropped))
    }

    /// Whether anything is subscribed at all.
    pub fn is_empty(&self) -> bool {
        self.subs.is_empty()
    }

    /// Recomputes the gate from the live subscriptions. The only place `wanted`
    /// and `detail` are written, so they cannot drift from the set they
    /// summarize.
    fn recompute(&mut self) {
        self.wanted = 0;
        self.detail = false;
        for sub in &self.subs {
            // An empty `kinds` means "every kind", which is every bit.
            self.wanted |= if sub.filter.kinds == 0 {
                u8::MAX
            } else {
                sub.filter.kinds
            };
            self.detail |= sub.detail;
        }
    }

    /// Offers a finished span to every subscription whose filter accepts it.
    ///
    /// Cloned per accepting subscription rather than shared, because each ring
    /// owns what it holds and a subscriber that closes must be able to drop its
    /// records without consulting the others.
    pub fn record(&mut self, record: SpanRecord) {
        if self.wanted == 0 {
            return;
        }
        let duration = record.ended_at - record.started_at;
        for sub in &mut self.subs {
            let filter = &sub.filter;
            if filter.kinds != 0 && filter.kinds & record.kind.bit() == 0 {
                continue;
            }
            if duration < filter.min_duration {
                continue;
            }
            if !sampled(record.trace_id.as_deref(), filter.sample) {
                continue;
            }
            // A `tick` record is never dropped. It is one per turn — bounded,
            // unlike everything else — and in an overloaded turn it is the
            // record that *explains* the overload: the turn ran long, and here
            // is how long. Drop-newest would make it the first casualty of
            // exactly the turn worth looking at, leaving a `dropped` count with
            // nothing to attribute it to.
            if record.kind != SpanKind::Tick && sub.buffer.len() >= filter.buffer_size {
                // Drop newest: what is already buffered stays coherent, and the
                // count makes the loss visible in the next batch.
                sub.dropped += 1;
                continue;
            }
            let mut record = record.clone();
            if !sub.detail {
                // Timings and kinds for an `observe`-only subscriber; no payload.
                record.attributes.clear();
                record.status_message = None;
            }
            sub.buffer.push_back(record);
        }
    }

    /// Takes each subscription's pending batch: `(subscription id, records,
    /// dropped since the last drain)`.
    ///
    /// Only subscriptions with something to say are returned, so a quiet tick
    /// hands back an empty `Vec` and the caller does nothing.
    pub(crate) fn drain(&mut self) -> Vec<(u64, Vec<SpanRecord>, u64)> {
        self.drain_from(Sink::Js)
    }

    /// The same, for one sink. A guest subscription and the exporter each hold
    /// their own copy of a record, so one closing or falling behind cannot
    /// affect the other.
    pub fn drain_from(&mut self, sink: Sink) -> Vec<(u64, Vec<SpanRecord>, u64)> {
        let mut batches = Vec::new();
        for sub in &mut self.subs {
            if sub.sink != sink || (sub.buffer.is_empty() && sub.dropped == 0) {
                continue;
            }
            let records: Vec<SpanRecord> = sub.buffer.drain(..).collect();
            batches.push((sub.id, records, std::mem::take(&mut sub.dropped)));
        }
        batches
    }
}

/// Whether a trace falls under a sampling threshold.
///
/// **Per trace, not per record**, and computed from the trace id rather than
/// from a stored decision: every record in a trace hashes the same id, so a
/// trace is whole or absent by construction. A decision cache would need
/// evicting, and evicting one mid-trace is exactly the half-sampled trace this
/// avoids. A record with no trace is always kept — it belongs to no trace that
/// could be cut in half.
fn sampled(trace: Option<&str>, threshold: u64) -> bool {
    if threshold == u64::MAX {
        return true;
    }
    let Some(trace) = trace else {
        return true;
    };
    hash(trace) <= threshold
}

/// FNV-1a. Not a cryptographic choice and does not need to be: it decides which
/// traces to keep, and the only property required is that the same id always
/// answers the same way and that ids spread evenly.
fn hash(value: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in value.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// Clones the recorder out of the isolate slot. Mirrors [`crate::op`]'s
/// `op_state`: the clone decouples access from the isolate borrow the scope
/// already holds.
pub(crate) fn recorder(
    scope: &v8::PinScope<'_, '_>,
) -> Option<std::rc::Rc<std::cell::RefCell<Recorder>>> {
    scope
        .get_slot::<std::rc::Rc<std::cell::RefCell<Recorder>>>()
        .cloned()
}

/// Everything a recording site needs to hold between a span's start and its end.
///
/// Built **only** when [`Recorder::wants`] said yes, so the fields that cost
/// something — the clock read, the id, the name — are never paid for by a
/// program that is not being watched.
pub(crate) struct OpenSpan {
    pub(crate) id: u64,
    pub(crate) parent_id: Option<u64>,
    pub(crate) trace_id: Option<Rc<str>>,
    pub(crate) name: Rc<str>,
    pub(crate) kind: SpanKind,
    pub(crate) scheduled_at: f64,
    pub(crate) started_at: f64,
    pub(crate) tick: u64,
}

impl OpenSpan {
    /// Opens a span, or `None` when nothing wants this kind.
    ///
    /// `scheduled_at` is when the work became *runnable* — equal to
    /// `started_at` where no queue is observable (see the module docs).
    pub(crate) fn open(
        recorder: &std::rc::Rc<std::cell::RefCell<Recorder>>,
        kind: SpanKind,
        name: impl FnOnce() -> Rc<str>,
        scope: impl FnOnce() -> (Option<u64>, Option<Rc<str>>),
        scheduled_at: Option<f64>,
    ) -> Option<OpenSpan> {
        let mut rec = recorder.borrow_mut();
        if !rec.wants(kind) {
            return None;
        }
        let started_at = rec.now();
        // The enclosing span and the trace, both read from the async capture, so
        // a record nests where the work actually happened rather than where the
        // call stack happens to be.
        let (parent_id, trace_id) = scope();
        Some(OpenSpan {
            id: rec.next_span_id(),
            parent_id,
            trace_id,
            name: name(),
            kind,
            // Clamped at `started_at`. A timer's deadline is reconstructed from
            // the engine's clock, while the scheduler anchors it at the reading
            // taken when the *turn* began — which is a fraction of a millisecond
            // earlier, so a firing can look infinitesimally "early". Queue delay
            // is a duration; a negative one is a reconstruction artefact and
            // reporting it would be worse than rounding it away.
            scheduled_at: scheduled_at.unwrap_or(started_at).min(started_at),
            started_at,
            tick: rec.tick(),
        })
    }

    /// Closes the span and offers it to the subscriptions.
    pub(crate) fn close(
        self,
        recorder: &std::rc::Rc<std::cell::RefCell<Recorder>>,
        status: SpanStatus,
        status_message: Option<Rc<str>>,
        attributes: Vec<(Rc<str>, AttrValue)>,
    ) {
        let mut rec = recorder.borrow_mut();
        let ended_at = rec.now();
        rec.record(SpanRecord {
            id: self.id,
            parent_id: self.parent_id,
            trace_id: self.trace_id,
            name: self.name,
            kind: self.kind,
            user: false,
            scheduled_at: self.scheduled_at,
            started_at: self.started_at,
            ended_at,
            status,
            status_message,
            attributes,
            tick: self.tick,
        });
    }
}

/// Marshals a record into the object `runtime:diagnostics` hands its subscriber.
///
/// Field names are OpenTelemetry's, unshortened, so an exporter attaches with no
/// translation layer.
pub fn record_to_value(record: &SpanRecord) -> crate::Value {
    use crate::Value;
    let attributes = Value::Object(
        record
            .attributes
            .iter()
            .map(|(key, value)| {
                let value = match value {
                    AttrValue::Str(s) => Value::String(s.to_string()),
                    AttrValue::Num(n) => Value::Number(*n),
                    AttrValue::Bool(b) => Value::Bool(*b),
                };
                (key.to_string(), value)
            })
            .collect(),
    );
    Value::Object(vec![
        ("id".to_string(), Value::Number(record.id as f64)),
        (
            "parentId".to_string(),
            match record.parent_id {
                Some(id) => Value::Number(id as f64),
                None => Value::Null,
            },
        ),
        (
            "traceId".to_string(),
            match &record.trace_id {
                Some(trace) => Value::String(trace.to_string()),
                None => Value::Null,
            },
        ),
        ("name".to_string(), Value::String(record.name.to_string())),
        (
            "kind".to_string(),
            Value::String(record.kind.name().to_string()),
        ),
        (
            "source".to_string(),
            Value::String(if record.user { "user" } else { "runtime" }.to_string()),
        ),
        (
            "scheduledAt".to_string(),
            Value::Number(record.scheduled_at),
        ),
        ("startedAt".to_string(), Value::Number(record.started_at)),
        ("endedAt".to_string(), Value::Number(record.ended_at)),
        (
            "status".to_string(),
            Value::String(record.status.name().to_string()),
        ),
        (
            "statusMessage".to_string(),
            match &record.status_message {
                Some(message) => Value::String(message.to_string()),
                None => Value::Null,
            },
        ),
        ("attributes".to_string(), attributes),
        ("tick".to_string(), Value::Number(record.tick as f64)),
    ])
}

/// Installs `__span_open` and `__span_close`: the runtime's own instrumentation
/// seam, used by `runtime:http` to open a span per inbound request.
///
/// # Why these are not capability-gated
///
/// The guest-facing `span()` in `runtime:diagnostics` is an *op* and is gated on
/// `diagnostics` like everything else in that module. These are different: they
/// exist so the **runtime** can instrument itself when the *deployment* turned on
/// telemetry with `--otel`, which the program did not ask for and holds no
/// capability for. Gating them would mean a deployment could only trace programs
/// that had granted themselves the right to be traced, which is backwards.
///
/// They are write-only and reveal nothing. A guest that calls them directly adds
/// a span to a recording it may well not be able to read — noise, not disclosure,
/// and the same noise it can already produce by making the runtime do work. They
/// are inert unless something is recording.
pub(crate) fn install_span_builtins(
    scope: &mut v8::PinScope,
    context: v8::Local<v8::Context>,
) -> crate::error::Result<()> {
    let global = context.global(scope);
    crate::op::install_global_fn(scope, global, "__span_open", span_open, None)?;
    crate::op::install_global_fn(scope, global, "__span_close", span_close, None)
}

/// The native callbacks this pair contributes to the snapshot's external
/// reference table.
pub(crate) fn span_external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![
        v8::ExternalReference {
            function: span_open.map_fn_to(),
        },
        v8::ExternalReference {
            function: span_close.map_fn_to(),
        },
    ]
}

fn span_open(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        span_open_inner(&mut *scope, args, rv);
    }));
}

/// `__span_open()` → `[spanId, startedAt]`, or `[0, 0]` when nothing is
/// recording.
fn span_open_inner(
    scope: &mut v8::PinScope,
    _args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let (id, started_at) = match recorder(scope) {
        Some(rec) => {
            let mut rec = rec.borrow_mut();
            // `is_recording`, not `wants`: the span has to exist so that what
            // happens inside it nests, even for a subscriber that filtered this
            // kind away.
            if rec.is_recording() {
                let now = rec.now();
                (rec.next_span_id(), now)
            } else {
                (0, 0.0)
            }
        }
        None => (0, 0.0),
    };
    let pair = [
        v8::Number::new(scope, id as f64).into(),
        v8::Number::new(scope, started_at).into(),
    ];
    rv.set(v8::Array::new_with_elements(scope, &pair).into());
}

fn span_close(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        span_close_inner(&mut *scope, args, rv);
    }));
}

/// `__span_close(id, parentId, name, startedAt, status, traceId, attrs)` —
/// records the span `__span_open` allocated.
///
/// `attrs` is a flat array of alternating key/value strings, which is all the
/// one caller needs and avoids marshaling an arbitrary object outside the op
/// boundary.
fn span_close_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue<v8::Value>,
) {
    let id = args.get(0).number_value(scope).unwrap_or(0.0);
    if id <= 0.0 {
        return;
    }
    let Some(rec) = recorder(scope) else {
        return;
    };
    let parent = args.get(1).number_value(scope).unwrap_or(-1.0);
    let name: Rc<str> = Rc::from(args.get(2).to_rust_string_lossy(scope));
    let started_at = args.get(3).number_value(scope).unwrap_or(0.0);
    let status = match args.get(4).to_rust_string_lossy(scope).as_str() {
        "error" => SpanStatus::Error,
        "cancelled" => SpanStatus::Cancelled,
        _ => SpanStatus::Ok,
    };
    let trace = args.get(5);
    let trace_id: Option<Rc<str>> = (!trace.is_undefined() && !trace.is_null())
        .then(|| Rc::from(trace.to_rust_string_lossy(scope)));

    let mut rec = rec.borrow_mut();
    if !rec.wants(SpanKind::Request) {
        return;
    }
    let attributes = if rec.wants_detail() {
        read_pairs(scope, args.get(6))
    } else {
        Vec::new()
    };
    let ended_at = rec.now();
    let tick = rec.tick();
    rec.record(SpanRecord {
        id: id as u64,
        parent_id: (parent >= 0.0).then_some(parent as u64),
        trace_id,
        status_message: None,
        name,
        kind: SpanKind::Request,
        // The runtime opened it, so it is not the program's.
        user: false,
        scheduled_at: started_at,
        started_at,
        ended_at,
        status,
        attributes,
        tick,
    });
}

/// Reads a flat `[k, v, k, v, …]` array of strings into attribute pairs.
fn read_pairs(
    scope: &mut v8::PinScope<'_, '_>,
    value: v8::Local<'_, v8::Value>,
) -> Vec<(Rc<str>, AttrValue)> {
    let Ok(array) = v8::Local::<v8::Array>::try_from(value) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    let mut i = 0;
    while i + 1 < array.length() {
        let (Some(key), Some(value)) = (array.get_index(scope, i), array.get_index(scope, i + 1))
        else {
            break;
        };
        pairs.push((
            Rc::from(key.to_rust_string_lossy(scope)),
            AttrValue::Str(Rc::from(value.to_rust_string_lossy(scope))),
        ));
        i += 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: SpanKind, trace: Option<&str>, started: f64, ended: f64) -> SpanRecord {
        SpanRecord {
            id: 1,
            parent_id: None,
            trace_id: trace.map(Rc::from),
            name: Rc::from("x"),
            kind,
            user: false,
            scheduled_at: started,
            started_at: started,
            ended_at: ended,
            status: SpanStatus::Ok,
            status_message: None,
            attributes: vec![(Rc::from("path"), AttrValue::Str(Rc::from("/etc/passwd")))],
            tick: 1,
        }
    }

    #[test]
    fn nothing_is_recorded_without_a_subscription() {
        let mut recorder = Recorder::new();
        for kind in SpanKind::ALL {
            assert!(!recorder.wants(kind), "{kind:?} wanted with no subscribers");
        }
        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        assert!(recorder.drain().is_empty());
    }

    #[test]
    fn a_filter_narrows_the_gate_to_its_kinds() {
        let mut recorder = Recorder::new();
        recorder.subscribe(
            Filter {
                kinds: SpanKind::Timer.bit(),
                ..Filter::default()
            },
            false,
        );
        assert!(recorder.wants(SpanKind::Timer));
        // The gate is what keeps an unwanted kind off the JS engine *and* off
        // the allocator: a recording site never builds the record at all.
        assert!(!recorder.wants(SpanKind::Op));

        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        assert!(recorder.drain().is_empty());
        recorder.record(record(SpanKind::Timer, None, 0.0, 1.0));
        assert_eq!(recorder.drain()[0].1.len(), 1);
    }

    #[test]
    fn an_empty_kinds_filter_wants_everything() {
        let mut recorder = Recorder::new();
        recorder.subscribe(Filter::default(), false);
        for kind in SpanKind::ALL {
            assert!(recorder.wants(kind), "{kind:?} should be wanted");
        }
    }

    #[test]
    fn min_duration_is_applied_host_side() {
        let mut recorder = Recorder::new();
        recorder.subscribe(
            Filter {
                min_duration: 10.0,
                ..Filter::default()
            },
            false,
        );
        recorder.record(record(SpanKind::Op, None, 0.0, 9.9));
        assert!(recorder.drain().is_empty());
        recorder.record(record(SpanKind::Op, None, 0.0, 10.0));
        assert_eq!(recorder.drain()[0].1.len(), 1);
    }

    #[test]
    fn attributes_need_detail() {
        let mut recorder = Recorder::new();
        recorder.subscribe(Filter::default(), false);
        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        let batch = recorder.drain();
        assert!(
            batch[0].1[0].attributes.is_empty(),
            "observe leaked payload"
        );

        let mut recorder = Recorder::new();
        recorder.subscribe(Filter::default(), true);
        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        let batch = recorder.drain();
        assert_eq!(batch[0].1[0].attributes.len(), 1);
    }

    #[test]
    fn two_subscriptions_see_their_own_filters() {
        let mut recorder = Recorder::new();
        let detailed = recorder.subscribe(Filter::default(), true);
        let plain = recorder.subscribe(
            Filter {
                kinds: SpanKind::Timer.bit(),
                ..Filter::default()
            },
            false,
        );
        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        let batches = recorder.drain();
        // Only the unfiltered one took the op, and it kept its payload.
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].0, detailed);
        assert_eq!(batches[0].1[0].attributes.len(), 1);
        // Closing the detailed one narrows the gate back.
        // Closing hands back what was buffered rather than discarding it.
        assert!(recorder.unsubscribe(detailed).is_some());
        assert!(!recorder.wants_detail());
        assert!(!recorder.wants(SpanKind::Op));
        recorder.unsubscribe(plain);
        assert!(recorder.unsubscribe(plain).is_none(), "closing twice");
        assert!(recorder.is_empty());
        assert!(!recorder.wants(SpanKind::Timer));
    }

    #[test]
    fn a_tick_record_survives_an_overflowing_buffer() {
        // The turn that overflowed is the turn worth seeing, and its own record
        // is what says so.
        let mut recorder = Recorder::new();
        recorder.subscribe(
            Filter {
                buffer_size: 2,
                ..Filter::default()
            },
            false,
        );
        for _ in 0..10 {
            recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        }
        recorder.record(record(SpanKind::Tick, None, 0.0, 40.0));
        let batch = recorder.drain();
        let (_, records, dropped) = &batch[0];
        assert!(
            records.iter().any(|r| r.kind == SpanKind::Tick),
            "the tick record was dropped from an overflowing buffer"
        );
        assert_eq!(*dropped, 8, "ordinary records still count as dropped");
    }

    #[test]
    fn overflow_drops_newest_and_counts_the_loss() {
        let mut recorder = Recorder::new();
        recorder.subscribe(
            Filter {
                buffer_size: 2,
                ..Filter::default()
            },
            false,
        );
        for i in 0..5 {
            let mut r = record(SpanKind::Op, None, 0.0, 1.0);
            r.id = i;
            recorder.record(r);
        }
        let batch = recorder.drain();
        let (_, records, dropped) = &batch[0];
        // The two that arrived first are the two that survived — a span's end is
        // never delivered without the start that was already buffered.
        assert_eq!(records.iter().map(|r| r.id).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(*dropped, 3);
        // The counter is reported once and then reset.
        recorder.record(record(SpanKind::Op, None, 0.0, 1.0));
        assert_eq!(recorder.drain()[0].2, 0);
    }

    #[test]
    fn sampling_keeps_a_trace_whole() {
        // Half sampling, and a trace that falls under it: every record of that
        // trace is kept, and every record of one that does not is dropped. A
        // per-record decision would split both.
        let half = u64::MAX / 2;
        let kept = (0..10_000)
            .map(|i| format!("{i:032x}"))
            .find(|t| sampled(Some(t), half))
            .expect("a trace under the threshold");
        let cut = (0..10_000)
            .map(|i| format!("{i:032x}"))
            .find(|t| !sampled(Some(t), half))
            .expect("a trace over the threshold");
        for _ in 0..50 {
            assert!(sampled(Some(&kept), half));
            assert!(!sampled(Some(&cut), half));
        }
        // A record belonging to no trace cannot be half of one.
        assert!(sampled(None, half));
        assert!(sampled(None, 0));
    }

    #[test]
    fn sampling_spreads_across_traces() {
        let half = u64::MAX / 2;
        let kept = (0..1000)
            .filter(|i| sampled(Some(&format!("{i:032x}")), half))
            .count();
        // Not a distribution test — just that the hash does not answer the same
        // way for everything, which a constant would.
        assert!((350..650).contains(&kept), "kept {kept} of 1000");
    }
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

/// What `metrics()` reports about garbage collection.
///
/// It sits beside [`LoopMetrics`] rather than in a section of its own because a
/// GC pause **is** loop lag: the loop is stopped for the whole of one, so a
/// major collection shows up in `lag` with no other explanation. Read together
/// they separate "the loop was idle" from "the loop was stopped".
#[derive(Clone, Debug, Default)]
pub struct GcMetrics {
    /// Collections so far — every kind, minor and major together.
    pub count: u64,
    /// How long each one stopped the isolate.
    pub pause: Histogram,
}

/// Per-isolate GC accounting.
///
/// A thread-local rather than an isolate slot, because the two V8 callbacks are
/// `extern "C"` and reaching a slot from one means turning the raw isolate
/// pointer back into a reference — unsafe for a number that a thread-local can
/// hold safely. One agent is one isolate on one thread (D48), so the two are the
/// same scope; a snapshot builder gets its own and never installs the hooks.
///
/// `Cell` and not `RefCell`: the prologue runs with an allocation in flight and
/// must not be able to panic, and a counter needs no borrow.
struct GcState {
    /// When the collection currently running began, or `None` between them.
    started_at: Cell<Option<Instant>>,
    metrics: RefCell<GcMetrics>,
}

thread_local! {
    static GC_STATE: GcState = GcState {
        started_at: Cell::new(None),
        metrics: RefCell::new(GcMetrics::default()),
    };
}

/// V8 is about to stop the isolate and collect.
unsafe extern "C" fn gc_prologue(
    _isolate: v8::UnsafeRawIsolatePtr,
    _kind: v8::GCType,
    _flags: v8::GCCallbackFlags,
    _data: *mut std::ffi::c_void,
) {
    GC_STATE.with(|state| state.started_at.set(Some(Instant::now())));
}

/// V8 has finished collecting and is about to resume the isolate.
unsafe extern "C" fn gc_epilogue(
    _isolate: v8::UnsafeRawIsolatePtr,
    _kind: v8::GCType,
    _flags: v8::GCCallbackFlags,
    _data: *mut std::ffi::c_void,
) {
    GC_STATE.with(|state| {
        let Some(started_at) = state.started_at.take() else {
            // An epilogue with no prologue: V8 nests incremental marking steps
            // inside a collection, so this is a step ending rather than a pause.
            return;
        };
        let pause = started_at.elapsed().as_secs_f64() * 1000.0;
        // `try_borrow_mut` because this runs at an arbitrary allocation point.
        // Nothing here should be able to end the program; a lost sample is a
        // sample, an abort is the program.
        if let Ok(mut metrics) = state.metrics.try_borrow_mut() {
            metrics.count += 1;
            metrics.pause.observe(pause);
        }
    });
}

/// Starts counting collections on this isolate.
///
/// Unconditional and not gated, like the loop histograms: two `Instant` reads
/// per collection is a fixed cost that does not scale with the program's work,
/// and `metrics()` is pull-only — a reader asking for the first time still wants
/// to see what the heap has been doing since before it asked. There is nothing
/// to subscribe to here that could turn it on in time.
pub fn install_gc_metrics(isolate: &mut v8::Isolate) {
    let null = std::ptr::null_mut();
    isolate.add_gc_prologue_callback(gc_prologue, null, v8::GCType::kGCTypeAll);
    isolate.add_gc_epilogue_callback(gc_epilogue, null, v8::GCType::kGCTypeAll);
}

/// What this isolate's heap has been doing.
pub fn gc_metrics() -> GcMetrics {
    GC_STATE.with(|state| state.metrics.borrow().clone())
}
