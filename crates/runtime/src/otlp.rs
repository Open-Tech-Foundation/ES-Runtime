//! OTLP/JSON encoding for `runtime:diagnostics` records (DECISIONS.md D89).
//!
//! The records were shaped to OpenTelemetry's field names from the start so that
//! an exporter would need no translation layer. This is the part that turns out
//! not to be free anyway — ids have to be hex, timestamps have to be absolute
//! nanoseconds, and a span has to say whether it is a server or a client — but
//! it is a mapping rather than a redesign, which is what naming the fields after
//! the standard bought.
//!
//! # Why JSON and not protobuf
//!
//! OTLP defines both, and `http/protobuf` is the more common default. Protobuf
//! would mean a schema, a code generator and a build step for a payload this
//! crate can write directly; JSON is a first-class OTLP protocol that every
//! collector accepts when the request says so. The trade is bytes on the wire,
//! paid by a deployment that opted into exporting. Protobuf is a later
//! optimisation, not a missing feature.
//!
//! # What is and is not exported
//!
//! Spans with a trace are exported. A `tick` record has none — it belongs to no
//! unit of work — so it is a *metric*, not a span, and is left out of the trace
//! payload. `metrics()` covers the same ground for a reader who wants it.

use es_runtime_engine::diagnostics::{AttrValue, SpanKind, SpanRecord, SpanStatus};

/// Converts the runtime's monotonic milliseconds into the absolute Unix
/// nanoseconds OTLP wants.
///
/// Held as the wall-clock instant the runtime's monotonic zero corresponds to,
/// captured once. Two clocks are needed because a span is timed on the monotonic
/// one — which never jumps — while a collector orders it against everything else
/// by wall time.
#[derive(Clone, Copy, Debug)]
pub struct TimeOrigin {
    /// Unix **nanoseconds** at monotonic zero, as an integer.
    ///
    /// Integer, and not `f64` milliseconds, because a Unix nanosecond is about
    /// 1.7e18 and an `f64` holds integers exactly only to 2^53 ≈ 9.0e15. Doing
    /// this arithmetic in floating point rounded every timestamp to the nearest
    /// ~128ns — small, but a span's start and end would drift apart by it, and a
    /// duration derived from two rounded ends is not the duration that was
    /// measured. `i64` reaches to the year 2262.
    wall_ns_at_zero: i64,
}

impl TimeOrigin {
    /// Captures the origin from a reading of both clocks taken together.
    ///
    /// Both must come from the same instant: the offset between them is what
    /// every exported timestamp is built on, so a gap between the two readings
    /// shifts the whole trace.
    pub fn new(wall_ms: u64, monotonic_ms: f64) -> TimeOrigin {
        TimeOrigin {
            wall_ns_at_zero: (wall_ms as i64).saturating_mul(1_000_000) - ns(monotonic_ms),
        }
    }

    /// Unix nanoseconds for a monotonic-millisecond reading, as the decimal
    /// string OTLP/JSON uses for 64-bit integers (a JSON *number* would lose the
    /// low digits past 2^53 in any consumer that parses it as a double).
    fn nanos(self, monotonic_ms: f64) -> String {
        (self.wall_ns_at_zero + ns(monotonic_ms)).max(0).to_string()
    }
}

/// Monotonic milliseconds as integer nanoseconds. Safe in `f64` because a
/// monotonic reading is an uptime, not an epoch — millions of years short of
/// where the precision runs out.
fn ns(monotonic_ms: f64) -> i64 {
    if !monotonic_ms.is_finite() {
        return 0;
    }
    (monotonic_ms * 1e6).round() as i64
}

/// Encodes a batch of records as an OTLP `ExportTraceServiceRequest` in JSON.
///
/// Returns `None` when the batch contains nothing exportable, so a quiet tick
/// costs no allocation and no request.
pub fn encode_traces(
    records: &[SpanRecord],
    service_name: &str,
    origin: TimeOrigin,
) -> Option<String> {
    let mut spans = String::new();
    let mut count = 0usize;
    for record in records {
        // No trace, no span: a `tick` belongs to no unit of work, and a trace
        // backend has nowhere to put one.
        let Some(trace_id) = record.trace_id.as_deref() else {
            continue;
        };
        if count > 0 {
            spans.push(',');
        }
        count += 1;
        encode_span(&mut spans, record, trace_id, origin);
    }
    if count == 0 {
        return None;
    }
    Some(format!(
        concat!(
            r#"{{"resourceSpans":[{{"resource":{{"attributes":[{{"key":"service.name","#,
            r#""value":{{"stringValue":{service}}}}}]}},"#,
            r#""scopeSpans":[{{"scope":{{"name":"esrun"}},"spans":[{spans}]}}]}}]}}"#
        ),
        service = json_string(service_name),
        spans = spans,
    ))
}

fn encode_span(out: &mut String, record: &SpanRecord, trace_id: &str, origin: TimeOrigin) {
    out.push_str(r#"{"traceId":"#);
    out.push_str(&json_string(trace_id));
    out.push_str(r#","spanId":"#);
    out.push_str(&json_string(&span_id(record.id)));
    if let Some(parent) = record.parent_id {
        out.push_str(r#","parentSpanId":"#);
        out.push_str(&json_string(&span_id(parent)));
    }
    out.push_str(r#","name":"#);
    out.push_str(&json_string(&record.name));
    out.push_str(r#","kind":"#);
    out.push_str(&otel_kind(record).to_string());
    out.push_str(r#","startTimeUnixNano":"#);
    out.push_str(&json_string(&origin.nanos(record.started_at)));
    out.push_str(r#","endTimeUnixNano":"#);
    out.push_str(&json_string(&origin.nanos(record.ended_at)));
    out.push_str(r#","attributes":["#);
    encode_attributes(out, record);
    out.push_str(r#"],"status":{"code":"#);
    out.push_str(match record.status {
        SpanStatus::Ok => "1",
        // OTel has no "cancelled": a span that did not finish its work did not
        // succeed, and collapsing it to `UNSET` would hide it from error rates.
        SpanStatus::Error | SpanStatus::Cancelled => "2",
    });
    out.push_str("}}");
}

/// OTel `SpanKind`: `INTERNAL=1`, `SERVER=2`, `CLIENT=3`.
///
/// An inbound request is a `SERVER` span; an op that reaches *out* of the process
/// — a fetch, a socket, a query — is a `CLIENT` one. Everything else is
/// `INTERNAL`, which is what OTel means by work that neither crosses a process
/// boundary nor is entered from outside.
fn otel_kind(record: &SpanRecord) -> u8 {
    match record.kind {
        SpanKind::Request => 2,
        SpanKind::Op if reaches_out(&record.name) => 3,
        _ => 1,
    }
}

fn reaches_out(name: &str) -> bool {
    name.starts_with("fetch")
        || name.starts_with("net_")
        || name.starts_with("ws_")
        || name.starts_with("db_")
}

/// Writes a record's attributes, mapping the generic `target` an op carries onto
/// the OpenTelemetry semantic convention for what that op acts on.
///
/// The host records one generic key because the convention it belongs to is a
/// property of the op, and threading a name through every ops module is a list
/// that goes stale. Doing it here, by family, keeps the knowledge in the one
/// place that has to know the standard anyway.
fn encode_attributes(out: &mut String, record: &SpanRecord) {
    let mut first = true;
    for (key, value) in &record.attributes {
        if !first {
            out.push(',');
        }
        first = false;
        let key: &str = if &**key == "target" {
            semantic_key(&record.name)
        } else {
            key
        };
        out.push_str(r#"{"key":"#);
        out.push_str(&json_string(key));
        out.push_str(r#","value":{"#);
        match value {
            AttrValue::Str(s) => {
                out.push_str(r#""stringValue":"#);
                out.push_str(&json_string(s));
            }
            AttrValue::Num(n) if n.fract() == 0.0 && n.is_finite() => {
                out.push_str(&format!(r#""intValue":"{n:.0}""#));
            }
            AttrValue::Num(n) => {
                let n = if n.is_finite() { *n } else { 0.0 };
                out.push_str(&format!(r#""doubleValue":{n}"#));
            }
            AttrValue::Bool(b) => {
                out.push_str(&format!(r#""boolValue":{b}"#));
            }
        }
        out.push_str("}}");
    }
}

/// The convention an op's `target` belongs to.
fn semantic_key(op: &str) -> &'static str {
    if op.starts_with("fs_") || op.starts_with("sync_fs_") {
        "file.path"
    } else if op.starts_with("db_") {
        "db.query.text"
    } else if op.starts_with("fetch") || op.starts_with("net_") || op.starts_with("ws_") {
        "url.full"
    } else if op.starts_with("system_") {
        "process.command"
    } else {
        // Honest rather than guessed: an op whose family has no convention keeps
        // the runtime's own name for the thing it acted on.
        "esrun.target"
    }
}

/// A span id as the 16 hex characters OTLP/JSON requires.
///
/// Our ids are a per-agent counter, which is unique where it has to be — within
/// a trace — and does not need to be unguessable: a span id is a correlation
/// key inside a payload the collector already trusts.
fn span_id(id: u64) -> String {
    format!("{id:016x}")
}

/// A JSON string literal. Hand-written because the alternative is a serde
/// dependency in the runtime for one string type, and because the escaping rules
/// are short enough to be obviously right.
fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything below 0x20 must be escaped; \u is the only form that
            // covers the ones without a short escape.
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    fn record(name: &str, kind: SpanKind) -> SpanRecord {
        SpanRecord {
            id: 7,
            parent_id: Some(3),
            trace_id: Some(Rc::from("0af7651916cd43dd8448eb211c80319c")),
            name: Rc::from(name),
            kind,
            user: false,
            scheduled_at: 10.0,
            started_at: 10.0,
            ended_at: 12.5,
            status: SpanStatus::Ok,
            attributes: Vec::new(),
            origin: 0,
            tick: 4,
        }
    }

    fn encode(records: &[SpanRecord]) -> String {
        encode_traces(records, "svc", TimeOrigin::new(1_700_000_000_000, 0.0))
            .expect("something to export")
    }

    #[test]
    fn a_span_carries_the_ids_otlp_requires() {
        let json = encode(&[record("fs_write", SpanKind::Op)]);
        assert!(json.contains(r#""traceId":"0af7651916cd43dd8448eb211c80319c""#));
        // 16 hex characters, zero-padded — not the bare integer.
        assert!(json.contains(r#""spanId":"0000000000000007""#), "{json}");
        assert!(
            json.contains(r#""parentSpanId":"0000000000000003""#),
            "{json}"
        );
        assert!(json.contains(r#""service.name""#));
    }

    #[test]
    fn a_root_span_omits_the_parent_rather_than_sending_zeros() {
        let mut r = record("GET", SpanKind::Request);
        r.parent_id = None;
        let json = encode(&[r]);
        assert!(!json.contains("parentSpanId"), "{json}");
    }

    #[test]
    fn timestamps_become_absolute_nanoseconds() {
        // Monotonic 10ms, with monotonic zero at Unix 1_700_000_000_000ms.
        let json = encode(&[record("fs_write", SpanKind::Op)]);
        assert!(
            json.contains(r#""startTimeUnixNano":"1700000000010000000""#),
            "{json}"
        );
        assert!(
            json.contains(r#""endTimeUnixNano":"1700000000012500000""#),
            "{json}"
        );
    }

    #[test]
    fn the_time_origin_is_taken_from_one_paired_reading() {
        // A runtime already 500ms old when the origin was captured still places
        // a span recorded at monotonic 10ms at wall 1_700_000_000_010.
        let origin = TimeOrigin::new(1_700_000_000_500, 500.0);
        assert_eq!(origin.nanos(10.0), "1700000000010000000");
    }

    #[test]
    fn span_kind_follows_which_way_the_work_crosses() {
        assert!(encode(&[record("GET", SpanKind::Request)]).contains(r#""kind":2"#));
        assert!(encode(&[record("fetch", SpanKind::Op)]).contains(r#""kind":3"#));
        assert!(encode(&[record("db_query", SpanKind::Op)]).contains(r#""kind":3"#));
        assert!(encode(&[record("fs_write", SpanKind::Op)]).contains(r#""kind":1"#));
        assert!(encode(&[record("work", SpanKind::User)]).contains(r#""kind":1"#));
    }

    #[test]
    fn status_maps_cancelled_onto_error() {
        let mut ok = record("x", SpanKind::Op);
        ok.status = SpanStatus::Ok;
        assert!(encode(&[ok]).contains(r#""status":{"code":1}"#));
        let mut err = record("x", SpanKind::Op);
        err.status = SpanStatus::Error;
        assert!(encode(&[err]).contains(r#""status":{"code":2}"#));
        // A span that was killed did not succeed; `UNSET` would hide it from an
        // error rate.
        let mut cancelled = record("x", SpanKind::Op);
        cancelled.status = SpanStatus::Cancelled;
        assert!(encode(&[cancelled]).contains(r#""status":{"code":2}"#));
    }

    #[test]
    fn a_generic_target_becomes_its_semantic_convention() {
        let target = |name: &str| {
            let mut r = record(name, SpanKind::Op);
            r.attributes = vec![(Rc::from("target"), AttrValue::Str(Rc::from("X")))];
            encode(&[r])
        };
        assert!(target("fs_write").contains(r#""key":"file.path""#));
        assert!(target("db_query").contains(r#""key":"db.query.text""#));
        assert!(target("fetch").contains(r#""key":"url.full""#));
        assert!(target("system_spawn").contains(r#""key":"process.command""#));
        // An op whose family has no convention keeps a namespaced key rather
        // than claiming one it does not fit.
        assert!(target("hash_digest").contains(r#""key":"esrun.target""#));
    }

    #[test]
    fn attributes_that_already_have_names_keep_them() {
        let mut r = record("GET", SpanKind::Request);
        r.attributes = vec![
            (
                Rc::from("http.request.method"),
                AttrValue::Str(Rc::from("GET")),
            ),
            (Rc::from("http.response.status_code"), AttrValue::Num(200.0)),
            (Rc::from("cached"), AttrValue::Bool(true)),
            (Rc::from("ratio"), AttrValue::Num(0.5)),
        ];
        let json = encode(&[r]);
        assert!(
            json.contains(r#"{"key":"http.request.method","value":{"stringValue":"GET"}}"#),
            "{json}"
        );
        // A whole number goes as an int, which is what a collector expects for a
        // status code; a fractional one keeps its precision.
        assert!(json.contains(r#""intValue":"200""#), "{json}");
        assert!(json.contains(r#""boolValue":true"#), "{json}");
        assert!(json.contains(r#""doubleValue":0.5"#), "{json}");
    }

    #[test]
    fn a_record_with_no_trace_is_not_a_span() {
        // A `tick` belongs to no unit of work, so it is a metric and a trace
        // backend has nowhere to put it.
        let mut tick = record("tick", SpanKind::Tick);
        tick.trace_id = None;
        assert!(encode_traces(&[tick], "svc", TimeOrigin::new(0, 0.0)).is_none());
        // And a batch of nothing costs no request at all.
        assert!(encode_traces(&[], "svc", TimeOrigin::new(0, 0.0)).is_none());
    }

    #[test]
    fn strings_are_escaped() {
        let mut r = record("weird", SpanKind::Op);
        r.attributes = vec![(
            Rc::from("target"),
            AttrValue::Str(Rc::from("a\"b\\c\nd\te\u{1}")),
        )];
        let json = encode(&[r]);
        // Checked escape by escape: one expected string would itself have to
        // contain a literal control character to be written down, which is
        // the thing under test.
        assert!(json.contains("a\\\"b"), "{json}");
        assert!(json.contains("b\\\\c"), "{json}");
        assert!(json.contains("c\\nd"), "{json}");
        assert!(json.contains("d\\te"), "{json}");
        assert!(json.contains("e\\u0001"), "{json}");
    }

    #[test]
    fn a_batch_of_several_is_one_payload() {
        let json = encode(&[
            record("GET", SpanKind::Request),
            record("fs_write", SpanKind::Op),
        ]);
        assert_eq!(json.matches(r#""traceId""#).count(), 2);
        assert_eq!(json.matches(r#""resourceSpans""#).count(), 1);
        assert_eq!(json.matches(r#""scopeSpans""#).count(), 1);
    }
}
