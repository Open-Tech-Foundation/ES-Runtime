//! Host ops backing the `runtime:serialization` module (XML/YAML/TOML/CSV/JSONL/MessagePack/CBOR).
use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// A deserialization visitor that builds V8 JavaScript objects from data.
pub struct ValueVisitor;

impl<'de> Visitor<'de> for ValueVisitor {
    type Value = Value;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("any valid JSON/YAML/TOML value")
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        Ok(Value::Bool(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        Ok(Value::Number(value as f64))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        Ok(Value::Number(value as f64))
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        Ok(Value::Number(value))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        Ok(Value::String(value.to_owned()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        Ok(Value::String(value))
    }

    fn visit_none<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut vec = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(elem) = seq.next_element_seed(ValueSeed)? {
            vec.push(elem);
        }
        Ok(Value::Array(vec))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut vec = Vec::with_capacity(map.size_hint().unwrap_or(0));
        while let Some(key) = map.next_key::<String>()? {
            let value = map.next_value_seed(ValueSeed)?;
            vec.push((key, value));
        }
        Ok(Value::Object(vec))
    }
}

#[derive(Clone, Copy)]
/// A deserialization seed that yields `Value` instances from a generic data format.
pub struct ValueSeed;

impl<'de> DeserializeSeed<'de> for ValueSeed {
    type Value = Value;

    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(ValueVisitor)
    }
}

/// `Number.MAX_SAFE_INTEGER`, the bound inside which an integer survives as a
/// JavaScript `number`.
const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// Whether an engine number is written as a TOML integer.
///
/// Engine numbers are always `f64`, so `1` and `1.0` are the same value, and an
/// integral one is written without the `.0`. Only inside the safe range,
/// though: `parse` reads an integer past it as a `BigInt` (TOML integers must be
/// lossless), so writing a large `number` as an integer would bring it back as
/// a different type. It is written as a float instead and reads back as the same
/// `number`. (An `i64` cast here also saturated, writing 2^63 as
/// `9223372036854775807`.)
fn as_integer(n: f64) -> Option<i64> {
    (n.fract() == 0.0 && n.abs() <= MAX_SAFE_INTEGER as f64).then_some(n as i64)
}

/// Converts an engine `Value` into a `toml::Value`, or reports what TOML cannot
/// represent.
///
/// Built directly rather than transcoded through `serde_json::Value`, for the
/// same reason [`toml_to_value`] is: a foreign `Value` type is free to use
/// private sentinels in its `Serialize` implementation, and those land in the
/// user's document. `serde_json` does exactly that under its
/// `arbitrary_precision` feature — every number becomes a one-key map named
/// `$serde_json::private::Number` — and a feature is not ours to control,
/// because Cargo unifies it across everything in the build. The fallible signature
/// is TOML's own: **there is no null**, so a null has to be refused rather than
/// written as something else. Routing through `serde_json::Value` used to get
/// that right by accident — the serializer rejected `Value::Null` — and this
/// says it deliberately, with a message that names the problem.
fn value_to_toml(v: Value) -> Result<toml::Value, OpError> {
    match v {
        Value::Bool(b) => Ok(toml::Value::Boolean(b)),
        // TOML floats include `nan`, `inf` and `-inf`, so every number fits.
        Value::Number(n) => Ok(match as_integer(n) {
            Some(i) => toml::Value::Integer(i),
            None => toml::Value::Float(n),
        }),
        // A `BigInt` is a TOML integer while it fits the 64 bits TOML integers
        // have.
        Value::BigInt(i) => i64::try_from(i)
            .map(toml::Value::Integer)
            .map_err(|_| OpError::type_error("TOML integers are 64-bit; this BigInt does not fit")),
        Value::String(s) => Ok(toml::Value::String(s)),
        Value::Array(arr) => Ok(toml::Value::Array(
            arr.into_iter()
                .map(value_to_toml)
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(map) => {
            let mut out = toml::map::Map::new();
            for (k, v) in map {
                out.insert(k, value_to_toml(v)?);
            }
            Ok(toml::Value::Table(out))
        }
        // TOML has no null, and nothing sensible to put in its place: dropping
        // the key would silently change the document.
        Value::Null => Err(OpError::type_error("TOML has no null")),
        Value::Undefined => Err(OpError::type_error(
            "TOML has no undefined (omit the key instead)",
        )),
        Value::Bytes(_) => Err(OpError::type_error(
            "TOML has no form for bytes (encode them as a string first)",
        )),
        _ => Err(OpError::type_error("value cannot be represented in TOML")),
    }
}

/// Converts a parsed `toml::Value` into an engine `Value`. TOML datetimes become
/// their RFC3339 string form — building Values directly (rather than transcoding
/// to JSON) avoids the `toml` crate leaking its `$__toml_private_datetime`
/// round-trip sentinel into user JS.
fn toml_to_value(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        // TOML 1.0: an integer must be read losslessly or refused. Past
        // 2^53 a `number` would round it, so it becomes a `BigInt` — the rule
        // CBOR follows (D170).
        toml::Value::Integer(i) if (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&i) => {
            Value::Number(i as f64)
        }
        toml::Value::Integer(i) => Value::BigInt(i128::from(i)),
        toml::Value::Float(f) => Value::Number(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_to_value).collect()),
        toml::Value::Table(t) => {
            Value::Object(t.into_iter().map(|(k, v)| (k, toml_to_value(v))).collect())
        }
    }
}

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    // XML: the reader, writer and stream all live in `crate::xml`, so the
    // three cannot drift apart (they used to walk events separately).
    engine.register_op(OpDecl::sync("xml_parse", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("xml_parse expects a string")),
        };
        // A parse failure throws (SyntaxError) rather than returning the error as
        // a string — otherwise a document that legitimately parses to a string
        // could be mistaken for an error by the caller.
        crate::xml::parse(xml)
            .map_err(|e| OpError::new(xml_class(e.kind), format!("Parse failed: {e}")))
    }))?;

    engine.register_op(OpDecl::sync("xml_validate", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        match crate::xml::validate(xml) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("xml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        crate::xml::emit(&val)
            .map(Value::String)
            .map_err(|e| OpError::new(xml_class(e.kind), format!("Build failed: {e}")))
    }))?;

    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    let xml_streams = Rc::new(RefCell::new(
        HashMap::<u32, crate::xml::StreamDecoder>::new(),
    ));
    let mut next_xml_stream = 1u32;

    let streams_new = Rc::clone(&xml_streams);
    engine.register_op(OpDecl::sync("xml_stream_new", move |_args| {
        let id = next_xml_stream;
        next_xml_stream += 1;
        streams_new
            .borrow_mut()
            .insert(id, crate::xml::StreamDecoder::new());
        Ok(Value::Number(id as f64))
    }))?;

    let streams_push = Rc::clone(&xml_streams);
    engine.register_op(OpDecl::sync("xml_stream_push", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let chunk = match args.get(1) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        let result = {
            let mut states = streams_push.borrow_mut();
            match states.get_mut(&id) {
                Some(state) => state.push(chunk),
                None => return Ok(Value::Array(vec![])),
            }
        };
        match result {
            Ok(records) => Ok(Value::Array(records)),
            Err(e) => {
                // A malformed record or an overflow ends the stream.
                streams_push.borrow_mut().remove(&id);
                Err(OpError::new(
                    xml_class(e.kind),
                    format!("Parse failed: {e}"),
                ))
            }
        }
    }))?;

    // Checks the end of the document (an unclosed root, no root at all) and
    // releases the state; the JS side also calls it on cancel, ignoring errors.
    let streams_close = Rc::clone(&xml_streams);
    engine.register_op(OpDecl::sync("xml_stream_close", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let Some(mut state) = streams_close.borrow_mut().remove(&id) else {
            return Ok(Value::Null);
        };
        state
            .finish()
            .map(|()| Value::Null)
            .map_err(|e| OpError::new(xml_class(e.kind), format!("Parse failed: {e}")))
    }))?;

    engine.register_op(OpDecl::sync("yaml_parse", |args| {
        let yaml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("yaml_parse expects a string")),
        };
        // Values are built directly rather than through a JSON string: JSON
        // cannot carry `.inf`/`.nan`, which would arrive as null.
        crate::yaml::parse(yaml).map_err(|e| {
            let class = match e.kind {
                crate::yaml::ErrorKind::Syntax => ExceptionClass::SyntaxError,
                crate::yaml::ErrorKind::Limit => ExceptionClass::RangeError,
            };
            OpError::new(class, format!("Parse failed: {e}"))
        })
    }))?;

    engine.register_op(OpDecl::sync("yaml_validate", |args| {
        let yaml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        match crate::yaml::validate(yaml) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("yaml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        crate::yaml::build(&val)
            .map(Value::String)
            .map_err(|e| OpError::type_error(format!("Build failed: {e}")))
    }))?;

    engine.register_op(OpDecl::sync("toml_parse", |args| {
        let toml_str = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("toml_parse expects a string")),
        };

        match toml::from_str::<toml::Value>(toml_str) {
            Ok(val) => Ok(toml_to_value(val)),
            Err(e) => Err(OpError::new(
                ExceptionClass::SyntaxError,
                format!("Parse failed: {e}"),
            )),
        }
    }))?;

    engine.register_op(OpDecl::sync("toml_validate", |args| {
        let toml_str = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };

        match toml::from_str::<toml::Value>(toml_str) {
            Ok(_) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {}", e))),
        }
    }))?;

    engine.register_op(OpDecl::sync("toml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        let toml_val = value_to_toml(val)?;

        // TOML requires the root to be an object (table)
        if !toml_val.is_table() {
            return Err(OpError::type_error(
                "TOML build requires the root to be an object",
            ));
        }

        match toml::to_string(&toml_val) {
            Ok(toml_str) => Ok(Value::String(toml_str)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("msgpack_parse", |args| {
        let msgpack_bytes = match args.first().and_then(Value::as_bytes) {
            Some(b) => b,
            None => return Err(OpError::type_error("msgpack_parse expects a Uint8Array")),
        };

        // JSON is the fast pivot (a string the guest's `JSON.parse` builds from,
        // which beats marshaling a value tree across the boundary) but it has no
        // byte string: routing `bin` through it turned every binary payload into
        // an array of numbers. Nor can it spell a non-string map key the way the
        // guest would. So the pivot is kept for documents that are JSON-shaped,
        // and only a document that carries `bin`/`ext` or a non-string key pays
        // for the value tree — one path per key, so the spelling cannot depend on
        // what else the document holds.
        match crate::msgpack::needs_value_tree(msgpack_bytes) {
            Ok(true) => {
                return crate::msgpack::read_value(msgpack_bytes).map_err(|e| {
                    OpError::new(ExceptionClass::SyntaxError, format!("Parse failed: {e}"))
                });
            }
            Ok(false) => {}
            // The scan is the one well-formedness check, so its error is the
            // error: the JSON reader below would accept an invalid `str` (as an
            // array of numbers), trailing bytes and deeper nesting.
            Err(e) => {
                return Err(OpError::new(
                    ExceptionClass::SyntaxError,
                    format!("Parse failed: {e}"),
                ));
            }
        }

        let mut deserializer = rmp_serde::Deserializer::new(msgpack_bytes);
        // Pre-allocate buffer aiming for an average 2x size increase
        let mut out = Vec::with_capacity(msgpack_bytes.len() * 2);
        let mut json_serializer = serde_json::Serializer::new(&mut out);

        match serde_transcode::transcode(&mut deserializer, &mut json_serializer) {
            Ok(_) => match String::from_utf8(out) {
                Ok(json_str) => Ok(Value::String(json_str)),
                Err(e) => Err(OpError::new(
                    ExceptionClass::SyntaxError,
                    format!("Invalid UTF-8 in JSON: {}", e),
                )),
            },
            Err(e) => Err(OpError::new(
                ExceptionClass::SyntaxError,
                format!("Parse failed: {}", e),
            )),
        }
    }))?;

    engine.register_op(OpDecl::sync("msgpack_validate", |args| {
        let msgpack_bytes = match args.first().and_then(Value::as_bytes) {
            Some(b) => b,
            None => return Ok(Value::String("Expected a Uint8Array".into())),
        };

        // The same check `decode` runs first, so the two cannot disagree.
        match crate::msgpack::validate(msgpack_bytes) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("msgpack_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        // Encoded straight from the value tree rather than through
        // `serde_json::Value`, which has no byte string: a `Uint8Array` used to
        // reach the encoder as JSON `null` and go out as `nil`, silently losing
        // the payload in the one format chosen for carrying it.
        match crate::msgpack::write_value(&val) {
            Ok(bytes) => Ok(Value::Bytes(bytes)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("cbor_parse", |args| {
        let cbor_bytes = match args.first().and_then(Value::as_bytes) {
            Some(b) => b,
            None => return Err(OpError::type_error("cbor_parse expects a Uint8Array")),
        };
        // CBOR decodes straight to the value tree, not through the JSON pivot:
        // JSON has neither a byte string nor `Infinity`/`NaN`, so a document
        // using a byte string or a non-finite float would be narrowed on the
        // way in. One structural walk avoids both.
        crate::cbor::read_value(cbor_bytes)
            .map_err(|e| OpError::new(ExceptionClass::SyntaxError, format!("Parse failed: {e}")))
    }))?;

    engine.register_op(OpDecl::sync("cbor_validate", |args| {
        let cbor_bytes = match args.first().and_then(Value::as_bytes) {
            Some(b) => b,
            None => return Ok(Value::String("Expected a Uint8Array".into())),
        };
        // `validate` walks the document without building the value tree, so a
        // validity check of untrusted input costs its own bytes rather than an
        // array or map the caller throws away. It accepts exactly what `decode`
        // accepts — a tag, a trailing byte, invalid UTF-8 are all still refused.
        match crate::cbor::validate(cbor_bytes) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("cbor_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        match crate::cbor::write_value(&val) {
            Ok(bytes) => Ok(Value::Bytes(bytes)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("csv_parse", |args| {
        let csv = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("csv_parse expects a string")),
        };
        let options = crate::csv::Options::from_value(args.get(1))
            .map_err(|e| OpError::new(csv_class(e.kind), e.message))?;
        crate::csv::parse(csv, &options)
            .map_err(|e| OpError::new(csv_class(e.kind), format!("Parse failed: {e}")))
    }))?;

    engine.register_op(OpDecl::sync("csv_validate", |args| {
        let csv = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };
        let options = match crate::csv::Options::from_value(args.get(1)) {
            Ok(o) => o,
            Err(e) => return Ok(Value::String(format!("Validation failed: {e}"))),
        };
        match crate::csv::validate(csv, &options) {
            Ok(()) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("csv_build", |args| {
        let value = args.first().cloned().unwrap_or(Value::Null);
        let options = crate::csv::Options::from_value(args.get(1))
            .map_err(|e| OpError::new(csv_class(e.kind), e.message))?;
        crate::csv::emit(&value, &options)
            .map(Value::String)
            .map_err(|e| OpError::new(csv_class(e.kind), format!("Build failed: {e}")))
    }))?;

    // Streaming decode/encode. The state lives here (like the XML stream) so a
    // chunk can be fed as it arrives; the JS side owns the `TransformStream`.
    // Unlike XML, `close` returns the flushed rows and can fail — a quote left
    // open is only knowable at the end — so the decoder's `flush` enqueues them.
    let csv_decoders = Rc::new(RefCell::new(
        HashMap::<u32, crate::csv::StreamDecoder>::new(),
    ));
    let csv_encoders = Rc::new(RefCell::new(
        HashMap::<u32, crate::csv::StreamEncoder>::new(),
    ));
    let mut next_decoder_id = 1u32;
    let mut next_encoder_id = 1u32;

    let decoders_new = Rc::clone(&csv_decoders);
    engine.register_op(OpDecl::sync("csv_stream_new", move |args| {
        let options = crate::csv::Options::from_value(args.first())
            .map_err(|e| OpError::new(csv_class(e.kind), e.message))?;
        let id = next_decoder_id;
        next_decoder_id += 1;
        decoders_new
            .borrow_mut()
            .insert(id, crate::csv::StreamDecoder::new(options));
        Ok(Value::Number(id as f64))
    }))?;

    let decoders_push = Rc::clone(&csv_decoders);
    engine.register_op(OpDecl::sync("csv_stream_push", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let chunk = match args.get(1) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };
        let result = {
            let mut states = decoders_push.borrow_mut();
            match states.get_mut(&id) {
                Some(state) => state.push(chunk),
                None => return Ok(Value::Array(vec![])),
            }
        };
        match result {
            Ok(rows) => Ok(Value::Array(rows)),
            Err(e) => {
                // A malformed row breaks the stream: drop it so the state cannot
                // be resumed into a second, inconsistent parse.
                decoders_push.borrow_mut().remove(&id);
                Err(OpError::new(
                    csv_class(e.kind),
                    format!("Parse failed: {e}"),
                ))
            }
        }
    }))?;

    let decoders_close = Rc::clone(&csv_decoders);
    engine.register_op(OpDecl::sync("csv_stream_close", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let mut state = match decoders_close.borrow_mut().remove(&id) {
            Some(state) => state,
            None => return Ok(Value::Array(vec![])),
        };
        match state.finish() {
            Ok(rows) => Ok(Value::Array(rows)),
            Err(e) => Err(OpError::new(
                csv_class(e.kind),
                format!("Parse failed: {e}"),
            )),
        }
    }))?;

    let encoders_new = Rc::clone(&csv_encoders);
    engine.register_op(OpDecl::sync("csv_encode_new", move |args| {
        let options = crate::csv::Options::from_value(args.first())
            .map_err(|e| OpError::new(csv_class(e.kind), e.message))?;
        let id = next_encoder_id;
        next_encoder_id += 1;
        encoders_new
            .borrow_mut()
            .insert(id, crate::csv::StreamEncoder::new(options));
        Ok(Value::Number(id as f64))
    }))?;

    let encoders_push = Rc::clone(&csv_encoders);
    engine.register_op(OpDecl::sync("csv_encode_push", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let row = args.get(1).cloned().unwrap_or(Value::Null);
        let result = {
            let mut states = encoders_push.borrow_mut();
            match states.get_mut(&id) {
                Some(state) => state.push(&row),
                None => return Ok(Value::String(String::new())),
            }
        };
        match result {
            Ok(text) => Ok(Value::String(text)),
            Err(e) => {
                encoders_push.borrow_mut().remove(&id);
                Err(OpError::new(
                    csv_class(e.kind),
                    format!("Build failed: {e}"),
                ))
            }
        }
    }))?;

    let encoders_close = Rc::clone(&csv_encoders);
    engine.register_op(OpDecl::sync("csv_encode_close", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        encoders_close.borrow_mut().remove(&id);
        Ok(Value::Null)
    }))?;

    Ok(())
}

/// Maps a CSV error kind to the JavaScript exception class it should raise.
fn csv_class(kind: crate::csv::ErrorKind) -> ExceptionClass {
    match kind {
        crate::csv::ErrorKind::Syntax => ExceptionClass::SyntaxError,
        crate::csv::ErrorKind::Type => ExceptionClass::TypeError,
        crate::csv::ErrorKind::Limit => ExceptionClass::RangeError,
    }
}

/// Maps an XML error kind to the JavaScript exception class it should raise.
fn xml_class(kind: crate::xml::ErrorKind) -> ExceptionClass {
    match kind {
        crate::xml::ErrorKind::Syntax => ExceptionClass::SyntaxError,
        crate::xml::ErrorKind::Type => ExceptionClass::TypeError,
        crate::xml::ErrorKind::Limit => ExceptionClass::RangeError,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml(src: &str) -> Value {
        toml_to_value(toml::from_str::<toml::Value>(src).expect("parse"))
    }

    #[test]
    fn toml_keeps_document_order() {
        let Value::Object(entries) = toml("zeta = 1\nalpha = 2\nmid = 3") else {
            panic!("expected a table")
        };
        let keys: Vec<&str> = entries.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["zeta", "alpha", "mid"]);
    }

    #[test]
    fn toml_integers_past_the_safe_range_are_bigints() {
        // TOML 1.0: integers are lossless or an error; an `f64` rounded these.
        assert_eq!(
            toml("a = 9007199254740993\nb = -9223372036854775808\nc = 9007199254740991"),
            Value::Object(vec![
                ("a".into(), Value::BigInt(9_007_199_254_740_993)),
                ("b".into(), Value::BigInt(i128::from(i64::MIN))),
                ("c".into(), Value::Number(9_007_199_254_740_991.0)),
            ])
        );
    }

    #[test]
    fn toml_build_writes_every_number_without_saturating() {
        let doc = |v: Value| {
            let t = value_to_toml(Value::Object(vec![("n".into(), v)])).expect("convert");
            toml::to_string(&t).expect("write")
        };
        assert_eq!(doc(Value::Number(42.0)), "n = 42\n");
        assert_eq!(doc(Value::Number(f64::NAN)), "n = nan\n");
        assert_eq!(doc(Value::Number(f64::NEG_INFINITY)), "n = -inf\n");
        assert_eq!(
            doc(Value::BigInt(i128::from(i64::MAX))),
            "n = 9223372036854775807\n"
        );
        // A large `number` is a float, so it reads back as a `number`, not as
        // the `BigInt` an integer that size would become — and not as
        // i64::MAX, which the old cast saturated to.
        let big = doc(Value::Number(2f64.powi(63)));
        assert!(big.ends_with(".0\n"), "{big}");
        assert_eq!(
            toml(&big),
            Value::Object(vec![("n".into(), Value::Number(2f64.powi(63)))])
        );
        assert!(value_to_toml(Value::BigInt(i128::from(i64::MAX) + 1)).is_err());
        assert!(value_to_toml(Value::Bytes(vec![1])).is_err());
    }

    #[test]
    fn toml_parses() {
        let toml_str = "a = 'hi'\nb = 42\n";
        let deserializer = toml::Deserializer::new(toml_str);
        let val = ValueSeed.deserialize(deserializer).unwrap();
        assert!(matches!(val, Value::Object(_)));
        if let Value::Object(map) = val {
            assert_eq!(map.len(), 2);
            assert_eq!(map[0].0, "a");
            assert!(matches!(map[0].1, Value::String(ref s) if s == "hi"));
            assert_eq!(map[1].0, "b");
            assert!(matches!(map[1].1, Value::Number(n) if n == 42.0));
        } else {
            panic!("Expected object");
        }
    }
}
