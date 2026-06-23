//! Host ops backing the `runtime:parsers` module for XML processing.
use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use quick_xml::Reader;
use quick_xml::events::Event;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use std::fmt;

/// Virtual filename used to compile an in-memory protobuf schema string.
const PROTOBUF_SCHEMA_FILE: &str = "schema.proto";

/// In-memory protobuf source resolver: serves a single virtual `schema.proto`
/// so `protox` can compile a schema string entirely in memory. `runtime:parsers`
/// declares no capability, so it must never touch the filesystem.
struct ProtoStringResolver {
    source: String,
}

impl protox::file::FileResolver for ProtoStringResolver {
    fn open_file(&self, name: &str) -> Result<protox::file::File, protox::Error> {
        if name == PROTOBUF_SCHEMA_FILE {
            protox::file::File::from_source(name, &self.source)
        } else {
            Err(protox::Error::file_not_found(name))
        }
    }
}

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

fn value_to_json(v: Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        // Engine numbers are always f64. Emit an integer JSON number for integral
        // values so builders don't turn `1` into `1.0` (YAML/TOML) or encode it as
        // a wasteful 9-byte float64 (MessagePack). Non-integral / out-of-range fall
        // back to a float; non-finite (no JSON representation) becomes null.
        Value::Number(n) => {
            if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                serde_json::Value::Number((n as i64).into())
            } else {
                serde_json::Number::from_f64(n)
                    .map(serde_json::Value::Number)
                    .unwrap_or(serde_json::Value::Null)
            }
        }
        Value::String(s) => serde_json::Value::String(s),
        Value::Array(arr) => serde_json::Value::Array(arr.into_iter().map(value_to_json).collect()),
        Value::Object(map) => {
            let mut obj = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                obj.insert(k, value_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        _ => serde_json::Value::Null,
    }
}

/// Converts a parsed `toml::Value` into an engine `Value`. TOML datetimes become
/// their RFC3339 string form — building Values directly (rather than transcoding
/// to JSON) avoids the `toml` crate leaking its `$__toml_private_datetime`
/// round-trip sentinel into user JS.
fn toml_to_value(v: toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::Number(i as f64),
        toml::Value::Float(f) => Value::Number(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.into_iter().map(toml_to_value).collect()),
        toml::Value::Table(t) => {
            Value::Object(t.into_iter().map(|(k, v)| (k, toml_to_value(v))).collect())
        }
    }
}

/// Streaming-decode state for `Protobuf.Schema.parseStream`: walks a buffered
/// protobuf message's wire bytes, yielding one element of a repeated message
/// field at a time so a huge collection never materializes all at once.
struct PbStream {
    bytes: Vec<u8>,
    cursor: usize,
    field_number: u32,
    element_desc: prost_reflect::MessageDescriptor,
}

/// Maximum element nesting accepted by the recursive XML reader. The parser
/// descends one stack frame per level, so an unbounded document (`<a><a>…`)
/// would otherwise overflow the stack and abort the process; past this depth we
/// fail gracefully instead. 256 matches libxml2's default and is far deeper than
/// any realistic document.
const MAX_XML_DEPTH: usize = 256;

fn parse_xml_node(
    reader: &mut Reader<&[u8]>,
    buf: &mut Vec<u8>,
    current_tag: Option<&[u8]>,
    depth: usize,
) -> Result<Value, String> {
    if depth > MAX_XML_DEPTH {
        return Err(format!(
            "Parse failed: XML nesting exceeds {MAX_XML_DEPTH} levels"
        ));
    }
    let mut map: Vec<(String, Value)> = Vec::new();
    let mut text_content = String::new();

    loop {
        match reader.read_event_into(buf) {
            Ok(Event::Start(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut child_map = Vec::new();
                for a in e.attributes().flatten() {
                    let k = format!("@{}", String::from_utf8_lossy(a.key.as_ref()));
                    let v = String::from_utf8_lossy(a.value.as_ref()).to_string();
                    child_map.push((k, Value::String(v)));
                }

                // `?` propagates a depth/parse error up instead of the old
                // `_ => {}` arm silently swallowing it.
                match parse_xml_node(reader, &mut Vec::new(), Some(e.name().as_ref()), depth + 1)? {
                    Value::Object(mut props) => child_map.append(&mut props),
                    Value::String(s) if !s.is_empty() => {
                        child_map.push(("$text".to_string(), Value::String(s)));
                    }
                    _ => {}
                }

                let child_val = Value::Object(child_map);
                if let Some(existing) = map.iter_mut().find(|(k, _)| k == &name) {
                    match &mut existing.1 {
                        Value::Array(arr) => arr.push(child_val),
                        _ => {
                            let old_val = std::mem::replace(&mut existing.1, Value::Null);
                            existing.1 = Value::Array(vec![old_val, child_val]);
                        }
                    }
                } else {
                    map.push((name, child_val));
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut child_map = Vec::new();
                for a in e.attributes().flatten() {
                    let k = format!("@{}", String::from_utf8_lossy(a.key.as_ref()));
                    let v = String::from_utf8_lossy(a.value.as_ref()).to_string();
                    child_map.push((k, Value::String(v)));
                }

                let child_val = if child_map.is_empty() {
                    Value::String("".to_string())
                } else {
                    Value::Object(child_map)
                };

                if let Some(existing) = map.iter_mut().find(|(k, _)| k == &name) {
                    match &mut existing.1 {
                        Value::Array(arr) => arr.push(child_val),
                        _ => {
                            let old_val = std::mem::replace(&mut existing.1, Value::Null);
                            existing.1 = Value::Array(vec![old_val, child_val]);
                        }
                    }
                } else {
                    map.push((name, child_val));
                }
            }
            Ok(Event::Text(e)) => {
                let text_str = String::from_utf8_lossy(e.as_ref());
                if let Ok(unescaped) = quick_xml::escape::unescape(&text_str) {
                    text_content.push_str(&unescaped);
                } else {
                    text_content.push_str(&text_str);
                }
            }
            Ok(Event::End(ref e)) if Some(e.name().as_ref()) == current_tag => {
                break;
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("Parse failed: {}", e)),
            _ => {}
        }
    }

    if map.is_empty() && !text_content.is_empty() {
        Ok(Value::String(text_content))
    } else {
        if !text_content.trim().is_empty() {
            map.push((
                "$text".to_string(),
                Value::String(text_content.trim().to_string()),
            ));
        }
        Ok(Value::Object(map))
    }
}

struct XmlStreamState {
    buffer: Vec<u8>,
    depth: usize,
}

/// Cap on a stream's retained (unconsumed) buffer. The decoder holds bytes until
/// a top-level element closes, re-scanning the tail on each push; an element
/// that never closes would otherwise grow without bound and turn the re-scan
/// quadratic. Past this, the stream fails instead of consuming unbounded memory.
const MAX_XML_STREAM_BUFFER: usize = 64 * 1024 * 1024;

/// Feeds one chunk into a streaming-decode state, returning any top-level
/// elements that completed. Pulled out of the op so the buffer cap is unit
/// testable. `Err` carries an overflow message (surfaced as a `RangeError`).
fn xml_stream_step(
    state: &mut XmlStreamState,
    chunk: &str,
    max_buffer: usize,
) -> Result<Vec<Value>, String> {
    state.buffer.extend_from_slice(chunk.as_bytes());

    let mut reader = Reader::from_reader(std::io::Cursor::new(&state.buffer));
    reader.config_mut().trim_text(true);

    let mut depth = state.depth;
    let mut start_pos = None;
    let mut consumed_bytes = 0;
    let mut depth_at_consumed = depth;
    let mut results = Vec::new();
    let mut buf = Vec::new();

    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => {
                if depth == 1 {
                    start_pos = Some(pos_before);
                }
                depth += 1;
            }
            Ok(Event::Empty(_)) if depth == 1 => {
                let end_pos = reader.buffer_position() as usize;
                let slice = &state.buffer[pos_before..end_pos];
                let mut sub_reader = Reader::from_reader(slice);
                sub_reader.config_mut().trim_text(true);
                if let Ok(val) = parse_xml_node(&mut sub_reader, &mut Vec::new(), None, 0) {
                    results.push(val);
                }
                consumed_bytes = end_pos;
                depth_at_consumed = depth;
            }
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 1 {
                    let end_pos = reader.buffer_position() as usize;
                    if let Some(start) = start_pos {
                        let slice = &state.buffer[start..end_pos];
                        let mut sub_reader = Reader::from_reader(slice);
                        sub_reader.config_mut().trim_text(true);
                        if let Ok(val) = parse_xml_node(&mut sub_reader, &mut Vec::new(), None, 0) {
                            results.push(val);
                        }
                        start_pos = None;
                    }
                    consumed_bytes = end_pos;
                    depth_at_consumed = depth;
                }
                if depth == 0 {
                    consumed_bytes = reader.buffer_position() as usize;
                    depth_at_consumed = depth;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break, // Incomplete tag, wait for more chunks
            _ => {}
        }
    }

    state.buffer.drain(..consumed_bytes);
    state.depth = depth_at_consumed;

    if state.buffer.len() > max_buffer {
        return Err(format!(
            "XML stream: unterminated element exceeds {max_buffer} bytes"
        ));
    }
    Ok(results)
}

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    engine.register_op(OpDecl::sync("xml_parse", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("xml_parse expects a string")),
        };

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut buf = Vec::new();
        // A parse failure throws (SyntaxError) rather than returning the error as
        // a string — otherwise a document that legitimately parses to a string
        // could be mistaken for an error by the caller.
        parse_xml_node(&mut reader, &mut buf, None, 0)
            .map_err(|e| OpError::new(ExceptionClass::SyntaxError, e))
    }))?;

    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;
    let stream_states = Rc::new(RefCell::new(HashMap::<u32, XmlStreamState>::new()));
    let mut next_stream_id = 1;

    let states_new = Rc::clone(&stream_states);
    engine.register_op(OpDecl::sync("xml_stream_new", move |_args| {
        let id = next_stream_id;
        next_stream_id += 1;
        states_new.borrow_mut().insert(
            id,
            XmlStreamState {
                buffer: Vec::new(),
                depth: 0,
            },
        );
        Ok(Value::Number(id as f64))
    }))?;

    let states_push = Rc::clone(&stream_states);
    engine.register_op(OpDecl::sync("xml_stream_push", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        let chunk = match args.get(1) {
            Some(Value::String(s)) => s.as_str(),
            _ => "",
        };

        let mut states = states_push.borrow_mut();
        let state = match states.get_mut(&id) {
            Some(s) => s,
            None => return Ok(Value::Array(vec![])),
        };

        match xml_stream_step(state, chunk, MAX_XML_STREAM_BUFFER) {
            Ok(results) => Ok(Value::Array(results)),
            Err(msg) => {
                // Overflow is fatal for this stream: drop its state and throw.
                states.remove(&id);
                Err(OpError::range_error(msg))
            }
        }
    }))?;

    let states_close = Rc::clone(&stream_states);
    engine.register_op(OpDecl::sync("xml_stream_close", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => 0,
        };
        states_close.borrow_mut().remove(&id);
        Ok(Value::Null)
    }))?;

    engine.register_op(OpDecl::sync("xml_validate", |args| {
        let xml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };

        let mut reader = Reader::from_str(xml);
        loop {
            match reader.read_event() {
                Ok(Event::Eof) => break,
                Err(e) => return Ok(Value::String(format!("Validation failed: {}", e))),
                _ => (),
            }
        }

        Ok(Value::Bool(true))
    }))?;

    engine.register_op(OpDecl::sync("xml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        let val = value_to_json(val);

        // Determine root tag: if it's an object with a single key, use it as root.
        // Otherwise use "root".
        let (root_name, inner_val) = match val.as_object() {
            Some(obj) if obj.len() == 1 => {
                let (k, v) = obj.into_iter().next().unwrap();
                (k.clone(), v.clone())
            }
            _ => ("root".to_string(), val),
        };

        match quick_xml::se::to_string_with_root(&root_name, &inner_val) {
            Ok(xml_str) => Ok(Value::String(xml_str)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("yaml_parse", |args| {
        let yaml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Err(OpError::type_error("yaml_parse expects a string")),
        };

        // Build engine Values directly rather than transcoding to a JSON string:
        // JSON can't represent `.inf`/`.nan`, so the transcode path would silently
        // turn non-finite floats into null. ValueSeed keeps them as JS Infinity/NaN.
        let deserializer = serde_yaml::Deserializer::from_str(yaml);
        match ValueSeed.deserialize(deserializer) {
            Ok(val) => Ok(val),
            Err(e) => Err(OpError::new(
                ExceptionClass::SyntaxError,
                format!("Parse failed: {e}"),
            )),
        }
    }))?;

    engine.register_op(OpDecl::sync("yaml_validate", |args| {
        let yaml = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => return Ok(Value::String("Expected a string".into())),
        };

        match serde_yaml::from_str::<serde_yaml::Value>(yaml) {
            Ok(_) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {}", e))),
        }
    }))?;

    engine.register_op(OpDecl::sync("yaml_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        let json_val = value_to_json(val);

        match serde_yaml::to_string(&json_val) {
            Ok(yaml_str) => Ok(Value::String(yaml_str)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
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
        let json_val = value_to_json(val);

        // TOML requires the root to be an object (table)
        if !json_val.is_object() {
            return Err(OpError::type_error(
                "TOML build requires the root to be an object",
            ));
        }

        match toml::to_string(&json_val) {
            Ok(toml_str) => Ok(Value::String(toml_str)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    engine.register_op(OpDecl::sync("msgpack_parse", |args| {
        let msgpack_bytes = match args.first().and_then(Value::as_bytes) {
            Some(b) => b,
            None => return Err(OpError::type_error("msgpack_parse expects a Uint8Array")),
        };

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

        match rmp_serde::from_slice::<serde::de::IgnoredAny>(msgpack_bytes) {
            Ok(_) => Ok(Value::Bool(true)),
            Err(e) => Ok(Value::String(format!("Validation failed: {}", e))),
        }
    }))?;

    engine.register_op(OpDecl::sync("msgpack_build", |mut args| {
        let val = args.drain(..).next().unwrap_or(Value::Null);
        let json_val = value_to_json(val);

        match rmp_serde::to_vec_named(&json_val) {
            Ok(bytes) => Ok(Value::Bytes(bytes)),
            Err(e) => Err(OpError::type_error(format!("Build failed: {e}"))),
        }
    }))?;

    // --- Protobuf Ops ---

    let protobuf_pools = Rc::new(RefCell::new(
        HashMap::<u32, prost_reflect::DescriptorPool>::new(),
    ));
    let mut next_protobuf_id = 1;

    let pools_create = Rc::clone(&protobuf_pools);
    engine.register_op(OpDecl::sync("protobuf_schema_create", move |args| {
        let proto_str = match args.first().and_then(Value::as_str) {
            Some(s) => s,
            None => {
                return Err(OpError::type_error(
                    "protobuf_schema_create expects a string schema",
                ));
            }
        };

        // Compile entirely in memory via a string resolver — no filesystem I/O
        // (this module holds no capability) and no cross-process temp-dir races.
        let resolver = ProtoStringResolver {
            source: proto_str.to_owned(),
        };
        let mut compiler = protox::Compiler::with_file_resolver(resolver);
        compiler.include_source_info(false);
        compiler.include_imports(true);
        if let Err(e) = compiler.open_file(PROTOBUF_SCHEMA_FILE) {
            return Err(OpError::new(
                ExceptionClass::SyntaxError,
                format!("Failed to compile schema: {e}"),
            ));
        }
        let pool = compiler.descriptor_pool();

        let id = next_protobuf_id;
        next_protobuf_id += 1;
        pools_create.borrow_mut().insert(id, pool);

        Ok(Value::Number(id as f64))
    }))?;

    // Release a compiled schema's descriptor pool. Without this a long-running
    // process that compiles schemas dynamically would grow the pool map forever.
    let pools_free = Rc::clone(&protobuf_pools);
    engine.register_op(OpDecl::sync("protobuf_schema_free", move |args| {
        if let Some(Value::Number(n)) = args.first() {
            pools_free.borrow_mut().remove(&(*n as u32));
        }
        Ok(Value::Null)
    }))?;

    let pools_parse = Rc::clone(&protobuf_pools);
    engine.register_op(OpDecl::sync("protobuf_parse", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => return Err(OpError::type_error("protobuf_parse expects schema id")),
        };
        let message_name = match args.get(1).and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => return Err(OpError::type_error("protobuf_parse expects message name")),
        };
        let payload = match args.get(2).and_then(Value::as_bytes) {
            Some(b) => b,
            None => {
                return Err(OpError::type_error(
                    "protobuf_parse expects a Uint8Array payload",
                ));
            }
        };

        let pools = pools_parse.borrow();
        let pool = pools
            .get(&id)
            .ok_or_else(|| OpError::type_error("Invalid schema ID"))?;
        let desc = pool.get_message_by_name(&message_name).ok_or_else(|| {
            OpError::type_error(format!("Message {} not found in schema", message_name))
        })?;

        let msg = prost_reflect::DynamicMessage::decode(desc, payload).map_err(|e| {
            OpError::new(
                ExceptionClass::SyntaxError,
                format!("Failed to decode protobuf payload: {e}"),
            )
        })?;

        // Serialize to a proto3-JSON string and let the JS side JSON.parse it.
        // V8's native JSON.parse builds the object graph far faster than marshaling
        // a Rust-side Value property-by-property across the FFI seam, which for a
        // large message (tens of thousands of objects) is dramatically slower.
        match serde_json::to_string(&msg) {
            Ok(json_str) => Ok(Value::String(json_str)),
            Err(e) => Err(OpError::type_error(format!(
                "Failed to transcode to JSON: {e}"
            ))),
        }
    }))?;

    let pools_build = Rc::clone(&protobuf_pools);
    engine.register_op(OpDecl::sync("protobuf_build", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => return Err(OpError::type_error("protobuf_build expects schema id")),
        };
        let message_name = match args.get(1).and_then(Value::as_str) {
            Some(s) => s.to_string(),
            None => return Err(OpError::type_error("protobuf_build expects message name")),
        };
        let val = args.get(2).cloned().unwrap_or(Value::Null);

        let pools = pools_build.borrow();
        let pool = pools
            .get(&id)
            .ok_or_else(|| OpError::type_error("Invalid schema ID"))?;
        let desc = pool.get_message_by_name(&message_name).ok_or_else(|| {
            OpError::type_error(format!("Message {} not found in schema", message_name))
        })?;

        let json_val = value_to_json(val);

        // `desc` (a MessageDescriptor) implements DeserializeSeed; serde_json::Value
        // implements serde::Deserializer — so this maps the JS object onto the message.
        let msg = match desc.deserialize(json_val) {
            Ok(m) => m,
            Err(e) => {
                return Err(OpError::type_error(format!(
                    "Failed to deserialize JSON to Protobuf: {}",
                    e
                )));
            }
        };

        use prost::Message;
        let mut bytes = Vec::new();
        if let Err(e) = msg.encode(&mut bytes) {
            return Err(OpError::type_error(format!(
                "Failed to encode Protobuf: {}",
                e
            )));
        }

        Ok(Value::Bytes(bytes))
    }))?;

    // --- Protobuf streaming (parseStream) ---
    //
    // Decodes a repeated *message* field element-by-element straight off the wire,
    // so a large collection (e.g. `repeated Book catalog`) is never fully decoded
    // into a single DynamicMessage tree + JSON string + JS object graph at once —
    // the consumer pulls one element at a time, bounding peak memory.
    let pb_streams = Rc::new(RefCell::new(HashMap::<u32, PbStream>::new()));
    let mut next_pb_stream_id = 1u32;

    let pools_stream = Rc::clone(&protobuf_pools);
    let streams_open = Rc::clone(&pb_streams);
    engine.register_op(OpDecl::sync("protobuf_stream_open", move |args| {
        let id = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => {
                return Err(OpError::type_error(
                    "protobuf_stream_open expects schema id",
                ));
            }
        };
        let message_name = args
            .get(1)
            .and_then(Value::as_str)
            .ok_or_else(|| OpError::type_error("protobuf_stream_open expects message name"))?;
        let field_name = args
            .get(2)
            .and_then(Value::as_str)
            .ok_or_else(|| OpError::type_error("protobuf_stream_open expects field name"))?;
        let payload = args.get(3).and_then(Value::as_bytes).ok_or_else(|| {
            OpError::type_error("protobuf_stream_open expects a Uint8Array payload")
        })?;

        let pools = pools_stream.borrow();
        let pool = pools
            .get(&id)
            .ok_or_else(|| OpError::type_error("Invalid schema ID"))?;
        let msg_desc = pool.get_message_by_name(message_name).ok_or_else(|| {
            OpError::type_error(format!("Message {message_name} not found in schema"))
        })?;
        let field = msg_desc.get_field_by_name(field_name).ok_or_else(|| {
            OpError::type_error(format!("Field {field_name} not found in {message_name}"))
        })?;
        if !field.is_list() {
            return Err(OpError::type_error(format!(
                "parseStream requires a repeated field; {field_name} is not repeated"
            )));
        }
        let element_desc = match field.kind() {
            prost_reflect::Kind::Message(m) => m,
            _ => {
                return Err(OpError::type_error(format!(
                    "parseStream supports only repeated message fields; {field_name} is scalar"
                )));
            }
        };
        let field_number = field.number();
        drop(pools);

        let sid = next_pb_stream_id;
        next_pb_stream_id = next_pb_stream_id.wrapping_add(1);
        streams_open.borrow_mut().insert(
            sid,
            PbStream {
                bytes: payload.to_vec(),
                cursor: 0,
                field_number,
                element_desc,
            },
        );
        Ok(Value::Number(f64::from(sid)))
    }))?;

    let streams_next = Rc::clone(&pb_streams);
    engine.register_op(OpDecl::sync("protobuf_stream_next", move |args| {
        use prost::encoding::{DecodeContext, WireType, decode_key, decode_varint, skip_field};

        let sid = match args.first() {
            Some(Value::Number(n)) => *n as u32,
            _ => {
                return Err(OpError::type_error(
                    "protobuf_stream_next expects stream id",
                ));
            }
        };
        // Take the state out so the cursor can advance without aliasing the map;
        // it is re-inserted only when an element is yielded (end/error drops it).
        let mut st = match streams_next.borrow_mut().remove(&sid) {
            Some(s) => s,
            None => return Ok(Value::Null),
        };

        let syntax = |e: &dyn std::fmt::Display| {
            OpError::new(
                ExceptionClass::SyntaxError,
                format!("Malformed protobuf stream: {e}"),
            )
        };

        loop {
            if st.cursor >= st.bytes.len() {
                return Ok(Value::Null); // exhausted — st dropped, freeing the buffer
            }
            let mut buf: &[u8] = &st.bytes[st.cursor..];
            let start = buf.len();
            let (tag, wire_type) = decode_key(&mut buf).map_err(|e| syntax(&e))?;

            if tag == st.field_number && wire_type == WireType::LengthDelimited {
                let len = decode_varint(&mut buf).map_err(|e| syntax(&e))? as usize;
                let header = start - buf.len();
                let payload_start = st.cursor + header;
                let payload_end = payload_start + len;
                if payload_end > st.bytes.len() {
                    return Err(syntax(&"length-delimited field overruns the buffer"));
                }
                let json = {
                    let element = &st.bytes[payload_start..payload_end];
                    let msg =
                        prost_reflect::DynamicMessage::decode(st.element_desc.clone(), element)
                            .map_err(|e| syntax(&e))?;
                    serde_json::to_string(&msg).map_err(|e| {
                        OpError::type_error(format!("Failed to transcode element to JSON: {e}"))
                    })?
                };
                st.cursor = payload_end;
                streams_next.borrow_mut().insert(sid, st);
                return Ok(Value::String(json));
            }

            // Not our field — skip its value and continue scanning.
            skip_field(wire_type, tag, &mut buf, DecodeContext::default())
                .map_err(|e| syntax(&e))?;
            st.cursor += start - buf.len();
        }
    }))?;

    let streams_close = Rc::clone(&pb_streams);
    engine.register_op(OpDecl::sync("protobuf_stream_close", move |args| {
        if let Some(Value::Number(n)) = args.first() {
            streams_close.borrow_mut().remove(&(*n as u32));
        }
        Ok(Value::Null)
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(xml: &str) -> Result<Value, String> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        parse_xml_node(&mut reader, &mut Vec::new(), None, 0)
    }

    #[test]
    fn normal_nesting_parses() {
        let v = parse("<a><b><c>hi</c></b></a>").expect("should parse");
        // a → b → c → "$text": "hi"
        assert!(matches!(v, Value::Object(_)));
    }

    #[test]
    fn nesting_within_limit_is_accepted() {
        // 256 levels deep — at the limit, still parses.
        let xml = format!(
            "{}{}",
            "<a>".repeat(MAX_XML_DEPTH),
            "</a>".repeat(MAX_XML_DEPTH)
        );
        assert!(parse(&xml).is_ok());
    }

    #[test]
    fn excessive_nesting_fails_gracefully() {
        // Far past the limit: returns an error instead of overflowing the stack.
        let n = MAX_XML_DEPTH + 5_000;
        let xml = format!("{}{}", "<a>".repeat(n), "</a>".repeat(n));
        let err = parse(&xml).expect_err("deep nesting must be rejected");
        assert!(err.contains("nesting exceeds"), "unexpected error: {err}");
    }

    #[test]
    fn stream_emits_top_level_elements_across_chunks() {
        let mut state = XmlStreamState {
            buffer: Vec::new(),
            depth: 0,
        };
        // Open the root and a partial child split mid-element across chunks.
        let r1 = xml_stream_step(&mut state, "<root><item>a</it", usize::MAX).unwrap();
        assert!(r1.is_empty(), "no element has closed yet");
        let r2 = xml_stream_step(&mut state, "em><item>b</item></root>", usize::MAX).unwrap();
        assert_eq!(r2.len(), 2, "both <item> elements emitted once closed");
        // Root consumed: nothing left to re-scan.
        assert!(state.buffer.is_empty());
    }

    #[test]
    fn stream_caps_unterminated_element() {
        let mut state = XmlStreamState {
            buffer: Vec::new(),
            depth: 0,
        };
        // A root that never closes, fed past a tiny cap → fails instead of
        // growing without bound.
        let err = xml_stream_step(&mut state, "<root>", 8)
            .and_then(|_| xml_stream_step(&mut state, "padding-text-that-never-closes", 8))
            .expect_err("unterminated element past the cap must error");
        assert!(err.contains("exceeds"), "unexpected error: {err}");
    }

    #[test]
    fn yaml_parses() {
        let yaml = "a: hi\nb: 42\n";
        // Since we test ops we typically test the function logic, but we can't easily call the op closure here directly in a unit test without an engine instance with the op registered.
        // Instead, let's just test ValueSeed.
        let deserializer = serde_yaml::Deserializer::from_str(yaml);
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
