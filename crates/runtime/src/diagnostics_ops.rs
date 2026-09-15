//! Host ops backing `runtime:diagnostics` (DECISIONS.md D89).
//!
//! The recorder itself lives in `engine` — it has to, because the things worth
//! recording are op dispatch, timer firing and the loop's own turns, all of
//! which happen there. What lives here is the **gate**: every op below names
//! [`Capability::DiagnosticsObserve`], and the two that carry payload out of the
//! program name [`Capability::DiagnosticsDetail`] as well. The security boundary
//! is the op, not the JS module (D7), which is exactly why these are ops and not
//! builtins like the `__ctx_*` accessors — a builtin would reach the recorder
//! with no check at all.
//!
//! # Why observability is gated
//!
//! Nothing here reaches outside the isolate, so it is not gated for the usual
//! reason. It is gated because it is authority over *the rest of the program*: a
//! library that could subscribe would learn every filesystem call, query and
//! request the process makes, how long each took, and — with `detail` — the
//! paths, URLs and SQL text involved. `observe` hands over timings and shapes;
//! `detail` hands over the data. They are separately grantable because those are
//! different disclosures, and a profiler only needs the first.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use es_runtime_common::Capability;
use es_runtime_engine::diagnostics::{
    AttrValue, Filter, Recorder, SpanKind, SpanRecord, SpanStatus,
};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::Clock;

use crate::Result;
use crate::handles::Inventory;

/// Installs the diagnostics ops, if the engine has a recorder to expose.
///
/// `inventory` is the list of this agent's handle registries — the same ones
/// D50's ownership check is built on, reused rather than duplicated, so what
/// `inventory()` reports cannot drift from what the runtime actually tracks.
pub(crate) fn install(
    engine: &mut dyn Engine,
    clock: Arc<dyn Clock>,
    inventory: Inventory,
) -> Result<()> {
    let Some(recorder) = engine.diagnostics() else {
        return Ok(());
    };

    // The engine owns no clock (D5), so it is handed one here. Monotonic
    // milliseconds with sub-millisecond precision — the same reading
    // `performance.now()` returns, so a user span and a runtime span are on one
    // timeline and can be compared without a conversion.
    let now = clock.clone();
    engine.enable_diagnostics(Rc::new(move || now.monotonic_micros() as f64 / 1_000.0));

    subscribe(engine, &recorder)?;
    unsubscribe(engine, &recorder)?;
    user_span(engine, &recorder)?;
    inventory_op(engine, &recorder, inventory)?;
    metrics(engine, &recorder)?;
    Ok(())
}

/// `diagnostics_subscribe(filter)` and `diagnostics_subscribe_detail(filter)` →
/// subscription id.
///
/// **Two ops rather than one with a flag.** Whether a subscription sees payloads
/// has to be decided by the capability check, and a boolean argument is decided
/// by the caller — JS could simply pass `true`. Registering the same work twice
/// behind two gates makes the refusal itself the answer: the module tries the
/// wider op and falls back when it is denied, and a run holding only `observe`
/// cannot reach the detail path at all.
fn subscribe(engine: &mut dyn Engine, recorder: &Rc<RefCell<Recorder>>) -> Result<()> {
    for (name, detail, capability) in [
        (
            "diagnostics_subscribe",
            false,
            Capability::DiagnosticsObserve,
        ),
        (
            "diagnostics_subscribe_detail",
            true,
            Capability::DiagnosticsDetail,
        ),
    ] {
        let recorder = recorder.clone();
        engine.register_op(
            OpDecl::sync(name, move |args| {
                let filter = parse_filter(args.first())?;
                Ok(Value::Number(
                    recorder.borrow_mut().subscribe(filter, detail) as f64,
                ))
            })
            .requires(capability),
        )?;
    }
    Ok(())
}

/// `diagnostics_unsubscribe(id)` → the subscription's final, undelivered batch.
///
/// Returned rather than dropped because delivery happens at the end of a turn,
/// and a subscriber closing inside one would otherwise lose everything recorded
/// in it — usually the work it subscribed to watch.
fn unsubscribe(engine: &mut dyn Engine, recorder: &Rc<RefCell<Recorder>>) -> Result<()> {
    let recorder = recorder.clone();
    engine.register_op(
        OpDecl::sync("diagnostics_unsubscribe", move |args| {
            let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
            let Some((records, dropped)) = recorder.borrow_mut().unsubscribe(id) else {
                return Ok(Value::Null);
            };
            Ok(Value::Object(vec![
                (
                    "records".to_string(),
                    Value::Array(
                        records
                            .iter()
                            .map(es_runtime_engine::diagnostics::record_to_value)
                            .collect(),
                    ),
                ),
                ("dropped".to_string(), Value::Number(dropped as f64)),
            ]))
        })
        .requires(Capability::DiagnosticsObserve),
    )?;
    Ok(())
}

/// `diagnostics_span_open()` → `[spanId, startedAt]`, or `[0, 0]` when nothing
/// is being recorded.
///
/// Split from the close so a span can be **current** for its own duration: the
/// module makes the returned id the enclosing span, and everything recorded
/// until it ends nests under it. Handing back the start time in the same call
/// saves a `performance.now()` — which is itself an op, and would otherwise
/// record a span inside every span.
///
/// The guest keeps the id and the start time; the host keeps no open-span table.
/// An unended span is then simply never recorded, rather than a host-side leak
/// that guest code controls.
fn user_span(engine: &mut dyn Engine, recorder: &Rc<RefCell<Recorder>>) -> Result<()> {
    let rec = recorder.clone();
    engine.register_op(
        OpDecl::sync("diagnostics_span_open", move |_args| {
            let mut rec = rec.borrow_mut();
            // `is_recording`, not `wants(User)`: an id is needed so the span can
            // be made current and nest what happens inside it, even for a
            // subscriber that filtered this kind away.
            if !rec.is_recording() {
                return Ok(Value::Array(vec![
                    Value::Number(0.0),
                    Value::Number(0.0),
                    Value::Number(-1.0),
                ]));
            }
            let now = rec.now();
            Ok(Value::Array(vec![
                Value::Number(rec.next_span_id() as f64),
                Value::Number(now),
            ]))
        })
        .requires(Capability::DiagnosticsObserve),
    )?;

    let recorder = recorder.clone();
    engine.register_op(
        OpDecl::sync("diagnostics_span_close", move |args| {
            let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
            if id == 0 {
                return Ok(Value::Undefined);
            }
            let parent = args.get(1).and_then(Value::as_number).unwrap_or(-1.0);
            let name: Rc<str> = Rc::from(args.get(2).and_then(Value::as_str).unwrap_or("span"));
            let started_at = args.get(3).and_then(Value::as_number).unwrap_or(0.0);
            let status = match args.get(4).and_then(Value::as_str) {
                Some("error") => SpanStatus::Error,
                Some("cancelled") => SpanStatus::Cancelled,
                _ => SpanStatus::Ok,
            };
            // The guest says which it is; `user` is the only one a program can
            // open for itself, and `runtime:http` opens `request`.
            let kind = match args.get(7).and_then(Value::as_str) {
                Some("request") => SpanKind::Request,
                _ => SpanKind::User,
            };
            let mut rec = recorder.borrow_mut();
            if !rec.wants(kind) {
                return Ok(Value::Undefined);
            }
            // Attributes are marshaled only when something paid for them. A
            // `detail` subscriber is rare; a `span()` in a hot loop is not.
            let attributes = if rec.wants_detail() {
                parse_attributes(args.get(5))
            } else {
                Vec::new()
            };
            let ended_at = rec.now();
            let tick = rec.tick();
            rec.record(SpanRecord {
                id,
                parent_id: (parent >= 0.0).then_some(parent as u64),
                trace_id: args.get(6).and_then(Value::as_str).map(Rc::from),
                name,
                kind,
                // `source: "user"` means the *program* opened it. A request span
                // is the runtime's, even though it comes through the same op.
                user: kind == SpanKind::User,
                // A user span is running from the moment it is opened; there is
                // no queue in front of it.
                scheduled_at: started_at,
                started_at,
                ended_at,
                status,
                attributes,
                tick,
            });
            Ok(Value::Undefined)
        })
        .requires(Capability::DiagnosticsObserve),
    )?;
    Ok(())
}

/// `diagnostics_inventory()` → `{ handles: [{ kind, count, ids }] }`.
fn inventory_op(
    engine: &mut dyn Engine,
    recorder: &Rc<RefCell<Recorder>>,
    inventory: Inventory,
) -> Result<()> {
    let _ = recorder;
    engine.register_op(
        OpDecl::sync("diagnostics_inventory", move |_args| {
            Ok(Value::Object(vec![(
                "handles".to_string(),
                Value::Array(inventory.snapshot()),
            )]))
        })
        .requires(Capability::DiagnosticsObserve),
    )?;
    Ok(())
}

/// `diagnostics_metrics()` → the pull-only counters.
fn metrics(engine: &mut dyn Engine, recorder: &Rc<RefCell<Recorder>>) -> Result<()> {
    let recorder = recorder.clone();
    engine.register_op(
        OpDecl::sync("diagnostics_metrics", move |_args| {
            let rec = recorder.borrow();
            let loop_metrics = rec.loop_metrics();
            Ok(Value::Object(vec![
                ("tick".to_string(), Value::Number(rec.tick() as f64)),
                (
                    "ticks".to_string(),
                    Value::Number(loop_metrics.ticks as f64),
                ),
                (
                    "tickDurationMs".to_string(),
                    histogram(&loop_metrics.duration),
                ),
                ("loopLagMs".to_string(), histogram(&loop_metrics.lag)),
            ]))
        })
        .requires(Capability::DiagnosticsObserve),
    )?;
    Ok(())
}

/// Marshals a histogram the way `metrics()` reports one.
fn histogram(h: &es_runtime_engine::diagnostics::Histogram) -> Value {
    Value::Object(vec![
        ("count".to_string(), Value::Number(h.count as f64)),
        ("min".to_string(), Value::Number(h.min)),
        ("max".to_string(), Value::Number(h.max)),
        ("mean".to_string(), Value::Number(h.mean())),
        ("p50".to_string(), Value::Number(h.quantile(0.50))),
        ("p99".to_string(), Value::Number(h.quantile(0.99))),
    ])
}

/// Reads the filter object the JS module has already validated. The refusals
/// here are the ones that must not depend on JS having behaved, since `__ops` is
/// reachable directly.
fn parse_filter(value: Option<&Value>) -> std::result::Result<Filter, OpError> {
    let mut filter = Filter::default();
    let Some(Value::Object(fields)) = value else {
        return Ok(filter);
    };
    let get = |name: &str| fields.iter().find(|(k, _)| k == name).map(|(_, v)| v);

    if let Some(Value::Array(kinds)) = get("kinds") {
        let mut mask = 0u8;
        for kind in kinds {
            let Some(name) = kind.as_str() else { continue };
            let Some(kind) = SpanKind::from_name(name) else {
                return Err(OpError::type_error(format!(
                    "unknown diagnostics kind {name:?}"
                )));
            };
            mask |= kind.bit();
        }
        filter.kinds = mask;
    }
    if let Some(min) = get("minDuration").and_then(Value::as_number) {
        if !min.is_finite() || min < 0.0 {
            return Err(OpError::range_error("minDuration must be >= 0"));
        }
        filter.min_duration = min;
    }
    if let Some(sample) = get("sample").and_then(Value::as_number) {
        if !(0.0..=1.0).contains(&sample) {
            return Err(OpError::range_error("sample must be between 0 and 1"));
        }
        // Held as the threshold the recording path compares a trace hash
        // against, so sampling costs no floating-point work per record.
        filter.sample = (sample * u64::MAX as f64) as u64;
    }
    if let Some(size) = get("bufferSize").and_then(Value::as_number) {
        if !size.is_finite() || size < 1.0 {
            return Err(OpError::range_error("bufferSize must be >= 1"));
        }
        filter.buffer_size = size as usize;
    }
    Ok(filter)
}

/// Reads an attributes object into the flat shape a record carries. Anything
/// that is not a string, number or boolean is dropped rather than stringified:
/// an exporter writes these into labels, and a label that says
/// `"[object Object]"` is worse than an absent one.
fn parse_attributes(value: Option<&Value>) -> Vec<(Rc<str>, AttrValue)> {
    let Some(Value::Object(fields)) = value else {
        return Vec::new();
    };
    fields
        .iter()
        .filter_map(|(key, value)| {
            let value = match value {
                Value::String(s) => AttrValue::Str(Rc::from(s.as_str())),
                Value::Number(n) => AttrValue::Num(*n),
                Value::Bool(b) => AttrValue::Bool(*b),
                _ => return None,
            };
            Some((Rc::from(key.as_str()), value))
        })
        .collect()
}
