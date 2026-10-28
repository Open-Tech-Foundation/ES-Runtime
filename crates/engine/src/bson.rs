//! `__bson_encode(value, types)` and `__bson_decode(bytes, types)`: BSON
//! between bytes and live JavaScript values (DECISIONS D187).
//!
//! # Why this lives in `engine`
//!
//! For the reason `serialize` does: an op sees a marshaled
//! [`Value`](crate::Value), so a document would be flattened into a value tree
//! on the way in and rebuilt from one on the way out. A MongoDB driver encodes
//! and decodes a document per operation, and that intermediate tree is the
//! per-operation garbage D146 and D147 removed. Here the encoder reads the
//! object where it is and the decoder builds objects straight from the bytes.
//! The bytes themselves are `es_runtime_bson`'s, which knows no engine type.
//!
//! # The BSON types
//!
//! `ObjectId`, `Decimal128` and the rest are JavaScript classes defined in
//! `runtime:serialization`. Each call passes them as `types`, an array of their
//! prototypes headed by the symbol their payload is stored under, so an instance
//! is recognized by one prototype comparison and its payload read with one
//! property lookup, with no call back into JavaScript per value. The order is
//! the table's: `ObjectId`, `Decimal128`, `UUID`, `Timestamp`, `Binary`, `Code`,
//! `BSONRegExp`, `MinKey`, `MaxKey`, `Double`.
//!
//! | Type | Payload |
//! | --- | --- |
//! | `ObjectId` | `[data]`: 24 hex digits |
//! | `Decimal128` | `[data]`: 32 hex digits of the stored bytes |
//! | `UUID` | `[data]`: 32 hex digits |
//! | `Timestamp` | `[data]`: a `bigint`, seconds high and increment low |
//! | `Binary` | `buffer` (a `Uint8Array`) and `subType` |
//! | `Code` | `code` and `scope` (an object or `null`) |
//! | `BSONRegExp` | `pattern` and `options` |
//! | `Double` | `value` |
//! | `MinKey`, `MaxKey` | none |

use es_runtime_bson::{Document, Element, MAX_DEPTH, Writer, kind};
use es_runtime_common::ExceptionClass;

use crate::convert::throw;
use crate::error::Result;
use crate::op::OpError;

pub(crate) fn install(scope: &mut v8::PinScope, context: v8::Local<v8::Context>) -> Result<()> {
    let global = context.global(scope);
    crate::op::install_global_fn(scope, global, "__bson_encode", bson_encode, None)?;
    crate::op::install_global_fn(scope, global, "__bson_decode", bson_decode, None)
}

pub(crate) fn external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![
        v8::ExternalReference {
            function: bson_encode.map_fn_to(),
        },
        v8::ExternalReference {
            function: bson_decode.map_fn_to(),
        },
    ]
}

/// The registered BSON classes, and the property names their payloads use.
struct Types<'s> {
    data: v8::Local<'s, v8::Symbol>,
    object_id: v8::Local<'s, v8::Value>,
    decimal128: v8::Local<'s, v8::Value>,
    uuid: v8::Local<'s, v8::Value>,
    timestamp: v8::Local<'s, v8::Value>,
    binary: v8::Local<'s, v8::Value>,
    code: v8::Local<'s, v8::Value>,
    regexp: v8::Local<'s, v8::Value>,
    min_key: v8::Local<'s, v8::Value>,
    max_key: v8::Local<'s, v8::Value>,
    double: v8::Local<'s, v8::Value>,
    object_prototype: v8::Local<'s, v8::Value>,
    names: Names<'s>,
}

struct Names<'s> {
    buffer: v8::Local<'s, v8::String>,
    sub_type: v8::Local<'s, v8::String>,
    code: v8::Local<'s, v8::String>,
    scope: v8::Local<'s, v8::String>,
    pattern: v8::Local<'s, v8::String>,
    options: v8::Local<'s, v8::String>,
    value: v8::Local<'s, v8::String>,
    flags: v8::Local<'s, v8::String>,
    reference: v8::Local<'s, v8::String>,
    id: v8::Local<'s, v8::String>,
}

impl<'s> Types<'s> {
    fn read(scope: &mut v8::PinScope<'s, '_>, value: v8::Local<'s, v8::Value>) -> Option<Self> {
        let list = v8::Local::<v8::Array>::try_from(value).ok()?;
        let at = |i: u32| list.get_index(scope, i);
        let data = v8::Local::<v8::Symbol>::try_from(at(0)?).ok()?;
        let types = [
            at(1)?,
            at(2)?,
            at(3)?,
            at(4)?,
            at(5)?,
            at(6)?,
            at(7)?,
            at(8)?,
            at(9)?,
            at(10)?,
        ];
        let object_prototype = v8::Object::new(scope).get_prototype(scope)?;
        let name = |scope: &mut v8::PinScope<'s, '_>, text: &str| {
            v8::String::new_from_one_byte(scope, text.as_bytes(), v8::NewStringType::Internalized)
        };
        let names = Names {
            buffer: name(scope, "buffer")?,
            sub_type: name(scope, "subType")?,
            code: name(scope, "code")?,
            scope: name(scope, "scope")?,
            pattern: name(scope, "pattern")?,
            options: name(scope, "options")?,
            value: name(scope, "value")?,
            flags: name(scope, "flags")?,
            reference: name(scope, "$ref")?,
            id: name(scope, "$id")?,
        };
        Some(Self {
            data,
            object_id: types[0],
            decimal128: types[1],
            uuid: types[2],
            timestamp: types[3],
            binary: types[4],
            code: types[5],
            regexp: types[6],
            min_key: types[7],
            max_key: types[8],
            double: types[9],
            object_prototype,
            names,
        })
    }

    /// An instance of one of the classes: its prototype, by identity.
    fn instance(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        prototype: v8::Local<'s, v8::Value>,
        names: &[v8::Local<'s, v8::Name>],
        values: &[v8::Local<'s, v8::Value>],
    ) -> Decoded<'s, v8::Object> {
        fast_object(scope, self.object_prototype, prototype, names, values)
    }
}

// ---- encode -----------------------------------------------------------------

/// Why an encode stopped. `Thrown` means JavaScript already threw (a getter
/// did) and that exception is left to propagate.
enum Fail {
    Thrown,
    Type(String),
    Range(String),
}

impl From<es_runtime_bson::Error> for Fail {
    fn from(e: es_runtime_bson::Error) -> Self {
        Fail::Range(e.to_string())
    }
}

type Encoded = std::result::Result<(), Fail>;

/// Where an element goes: a named field, or an array position.
#[derive(Clone, Copy)]
enum Slot<'k> {
    Name(&'k str),
    Index(usize),
}

impl Slot<'_> {
    fn write(self, w: &mut Writer, tag: u8) -> Encoded {
        match self {
            Slot::Name(name) => w.key(tag, name).map_err(|e| Fail::Type(e.to_string())),
            Slot::Index(i) => {
                w.index(tag, i);
                Ok(())
            }
        }
    }
}

struct Encoder<'t, 's> {
    types: &'t Types<'s>,
    w: Writer,
    /// The objects being written, outermost first, to refuse a cycle.
    ancestors: Vec<v8::Local<'s, v8::Object>>,
}

impl<'s> Encoder<'_, 's> {
    fn document(
        &mut self,
        scope: &mut v8::PinScope<'s, '_>,
        object: v8::Local<'s, v8::Object>,
    ) -> Encoded {
        if self.ancestors.contains(&object) {
            return Err(Fail::Type("a document cannot contain itself".into()));
        }
        self.ancestors.push(object);
        self.w.start_document()?;
        if object.is_map() {
            let map = v8::Local::<v8::Map>::try_from(object).map_err(|_| Fail::Thrown)?;
            let pairs = map.as_array(scope);
            let mut i = 0;
            while i + 1 < pairs.length() {
                let key = pairs.get_index(scope, i).ok_or(Fail::Thrown)?;
                let value = pairs.get_index(scope, i + 1).ok_or(Fail::Thrown)?;
                if !key.is_string() {
                    return Err(Fail::Type(
                        "a Map's keys must be strings to become a document".into(),
                    ));
                }
                let key = key.to_rust_string_lossy(scope);
                if !value.is_undefined() {
                    self.value(scope, Slot::Name(&key), value)?;
                }
                i += 2;
            }
        } else {
            let keys = object
                .get_own_property_names(
                    scope,
                    v8::GetPropertyNamesArgs {
                        mode: v8::KeyCollectionMode::OwnOnly,
                        property_filter: v8::PropertyFilter::ONLY_ENUMERABLE
                            | v8::PropertyFilter::SKIP_SYMBOLS,
                        index_filter: v8::IndexFilter::IncludeIndices,
                        key_conversion: v8::KeyConversionMode::ConvertToString,
                    },
                )
                .ok_or(Fail::Thrown)?;
            for i in 0..keys.length() {
                let key = keys.get_index(scope, i).ok_or(Fail::Thrown)?;
                let value = object.get(scope, key).ok_or(Fail::Thrown)?;
                // Absent rather than null, as JSON omits it.
                if value.is_undefined() {
                    continue;
                }
                let key = key.to_rust_string_lossy(scope);
                self.value(scope, Slot::Name(&key), value)?;
            }
        }
        self.w.end_document()?;
        self.ancestors.pop();
        Ok(())
    }

    fn array(
        &mut self,
        scope: &mut v8::PinScope<'s, '_>,
        array: v8::Local<'s, v8::Array>,
    ) -> Encoded {
        let object: v8::Local<v8::Object> = array.into();
        if self.ancestors.contains(&object) {
            return Err(Fail::Type("an array cannot contain itself".into()));
        }
        self.ancestors.push(object);
        self.w.start_document()?;
        for i in 0..array.length() {
            let value = array.get_index(scope, i).ok_or(Fail::Thrown)?;
            // A hole or `undefined` in an array is null, as in JSON: dropping it
            // would shift every later position.
            let slot = Slot::Index(usize::try_from(i).unwrap_or(usize::MAX));
            if value.is_undefined() {
                slot.write(&mut self.w, kind::NULL)?;
            } else {
                self.value(scope, slot, value)?;
            }
        }
        self.w.end_document()?;
        self.ancestors.pop();
        Ok(())
    }

    fn value(
        &mut self,
        scope: &mut v8::PinScope<'s, '_>,
        slot: Slot<'_>,
        value: v8::Local<'s, v8::Value>,
    ) -> Encoded {
        if value.is_null() || value.is_undefined() {
            return slot.write(&mut self.w, kind::NULL);
        }
        if value.is_boolean() {
            slot.write(&mut self.w, kind::BOOLEAN)?;
            self.w.boolean(value.is_true());
            return Ok(());
        }
        if value.is_int32() {
            slot.write(&mut self.w, kind::INT32)?;
            self.w.int32(value.int32_value(scope).unwrap_or(0));
            return Ok(());
        }
        if value.is_number() {
            slot.write(&mut self.w, kind::DOUBLE)?;
            self.w.double(value.number_value(scope).unwrap_or(f64::NAN));
            return Ok(());
        }
        if value.is_string() {
            let text = value.to_rust_string_lossy(scope);
            slot.write(&mut self.w, kind::STRING)?;
            return Ok(self.w.string(&text)?);
        }
        if value.is_big_int() {
            let big = v8::Local::<v8::BigInt>::try_from(value).map_err(|_| Fail::Thrown)?;
            let (n, lossless) = big.i64_value();
            if !lossless {
                return Err(Fail::Range(format!(
                    "{} does not fit in a BSON int64",
                    value.to_rust_string_lossy(scope)
                )));
            }
            slot.write(&mut self.w, kind::INT64)?;
            self.w.int64(n);
            return Ok(());
        }
        let Ok(object) = v8::Local::<v8::Object>::try_from(value) else {
            let what = if value.is_symbol() {
                "a symbol"
            } else {
                "this value"
            };
            return Err(Fail::Type(format!("{what} cannot be encoded as BSON")));
        };
        if object.is_function() {
            return Err(Fail::Type("a function cannot be encoded as BSON".into()));
        }
        if value.is_date() {
            let date = v8::Local::<v8::Date>::try_from(value).map_err(|_| Fail::Thrown)?;
            let ms = date.value_of();
            if !ms.is_finite() {
                return Err(Fail::Range(
                    "an Invalid Date cannot be encoded as BSON".into(),
                ));
            }
            slot.write(&mut self.w, kind::DATETIME)?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "a Date's time value is an integer within ±8.64e15"
            )]
            self.w.int64(ms as i64);
            return Ok(());
        }
        if value.is_reg_exp() {
            let regexp = v8::Local::<v8::RegExp>::try_from(value).map_err(|_| Fail::Thrown)?;
            let pattern = regexp.get_source(scope).to_rust_string_lossy(scope);
            let flags = object
                .get(scope, self.types.names.flags.into())
                .ok_or(Fail::Thrown)?
                .to_rust_string_lossy(scope);
            // The flags BSON and the server share; `g`, `y` and `d` change how a
            // JavaScript match runs, not what the pattern means.
            let options: String = flags
                .chars()
                .filter_map(|c| match c {
                    'i' | 'm' | 's' | 'u' => Some(c),
                    'v' => Some('u'),
                    _ => None,
                })
                .collect();
            slot.write(&mut self.w, kind::REGEX)?;
            return self
                .w
                .regex(&pattern, &options)
                .map_err(|e| Fail::Type(e.to_string()));
        }
        if value.is_uint8_array() || value.is_array_buffer() {
            let bytes = bytes_of(scope, value).unwrap_or_default();
            slot.write(&mut self.w, kind::BINARY)?;
            return Ok(self.w.binary(0, &bytes)?);
        }
        if value.is_array_buffer_view() {
            return Err(Fail::Type(
                "only a Uint8Array or ArrayBuffer encodes as BSON binary; use BSON.Binary for a vector or another subtype".into(),
            ));
        }
        if value.is_array() {
            let array = v8::Local::<v8::Array>::try_from(value).map_err(|_| Fail::Thrown)?;
            slot.write(&mut self.w, kind::ARRAY)?;
            return self.array(scope, array);
        }
        if value.is_set() || value.is_weak_map() || value.is_weak_set() || value.is_promise() {
            return Err(Fail::Type("this object cannot be encoded as BSON".into()));
        }

        let prototype = object.get_prototype(scope).ok_or(Fail::Thrown)?;
        if prototype != self.types.object_prototype
            && let Some(done) = self.typed(scope, slot, object, prototype)?
        {
            return Ok(done);
        }
        slot.write(&mut self.w, kind::DOCUMENT)?;
        self.document(scope, object)
    }

    /// One of the BSON classes, or `None` for any other object.
    fn typed(
        &mut self,
        scope: &mut v8::PinScope<'s, '_>,
        slot: Slot<'_>,
        object: v8::Local<'s, v8::Object>,
        prototype: v8::Local<'s, v8::Value>,
    ) -> std::result::Result<Option<()>, Fail> {
        let t = self.types;
        let data =
            |scope: &mut v8::PinScope<'s, '_>| object.get(scope, t.data.into()).ok_or(Fail::Thrown);
        let field = |scope: &mut v8::PinScope<'s, '_>, name: v8::Local<'s, v8::String>| {
            object.get(scope, name.into()).ok_or(Fail::Thrown)
        };
        let corrupt = |what: &str| Fail::Type(format!("this {what} has lost its value"));

        if prototype == t.object_id {
            let hex = data(scope)?.to_rust_string_lossy(scope);
            let bytes = es_runtime_bson::from_hex::<12>(&hex).ok_or_else(|| corrupt("ObjectId"))?;
            slot.write(&mut self.w, kind::OBJECT_ID)?;
            self.w.raw(&bytes);
        } else if prototype == t.decimal128 {
            let hex = data(scope)?.to_rust_string_lossy(scope);
            let bytes =
                es_runtime_bson::from_hex::<16>(&hex).ok_or_else(|| corrupt("Decimal128"))?;
            slot.write(&mut self.w, kind::DECIMAL128)?;
            self.w.raw(&bytes);
        } else if prototype == t.uuid {
            let hex = data(scope)?.to_rust_string_lossy(scope);
            let bytes = es_runtime_bson::from_hex::<16>(&hex).ok_or_else(|| corrupt("UUID"))?;
            slot.write(&mut self.w, kind::BINARY)?;
            self.w.binary(4, &bytes)?;
        } else if prototype == t.timestamp {
            let value = data(scope)?;
            let big = v8::Local::<v8::BigInt>::try_from(value).map_err(|_| corrupt("Timestamp"))?;
            let (n, lossless) = big.u64_value();
            if !lossless {
                return Err(corrupt("Timestamp"));
            }
            slot.write(&mut self.w, kind::TIMESTAMP)?;
            self.w.timestamp(n);
        } else if prototype == t.binary {
            let buffer = field(scope, t.names.buffer)?;
            let sub_type = field(scope, t.names.sub_type)?;
            let bytes = if buffer.is_uint8_array() {
                bytes_of(scope, buffer)
            } else {
                None
            }
            .ok_or_else(|| Fail::Type("a Binary's buffer must be a Uint8Array".into()))?;
            let sub_type = sub_type
                .is_uint32()
                .then(|| sub_type.uint32_value(scope))
                .flatten()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| {
                    Fail::Range("a Binary's subType must be an integer from 0 to 255".into())
                })?;
            slot.write(&mut self.w, kind::BINARY)?;
            self.w.binary(sub_type, &bytes)?;
        } else if prototype == t.code {
            let code = field(scope, t.names.code)?.to_rust_string_lossy(scope);
            let code_scope = field(scope, t.names.scope)?;
            match v8::Local::<v8::Object>::try_from(code_scope) {
                Ok(code_scope) if !code_scope.is_null() => {
                    slot.write(&mut self.w, kind::CODE_WITH_SCOPE)?;
                    self.w.start_code_with_scope(&code)?;
                    self.document(scope, code_scope)?;
                    self.w.end_code_with_scope()?;
                }
                _ => {
                    slot.write(&mut self.w, kind::CODE)?;
                    self.w.string(&code)?;
                }
            }
        } else if prototype == t.regexp {
            let pattern = field(scope, t.names.pattern)?.to_rust_string_lossy(scope);
            let options = field(scope, t.names.options)?.to_rust_string_lossy(scope);
            slot.write(&mut self.w, kind::REGEX)?;
            self.w
                .regex(&pattern, &options)
                .map_err(|e| Fail::Type(e.to_string()))?;
        } else if prototype == t.min_key {
            slot.write(&mut self.w, kind::MIN_KEY)?;
        } else if prototype == t.max_key {
            slot.write(&mut self.w, kind::MAX_KEY)?;
        } else if prototype == t.double {
            let value = field(scope, t.names.value)?;
            let n = value
                .is_number()
                .then(|| value.number_value(scope))
                .flatten()
                .ok_or_else(|| Fail::Type("a Double's value must be a number".into()))?;
            slot.write(&mut self.w, kind::DOUBLE)?;
            self.w.double(n);
        } else {
            return Ok(None);
        }
        Ok(Some(()))
    }
}

/// A copy of the bytes of a view or an `ArrayBuffer`.
fn bytes_of(scope: &mut v8::PinScope<'_, '_>, value: v8::Local<'_, v8::Value>) -> Option<Vec<u8>> {
    let view = match v8::Local::<v8::ArrayBuffer>::try_from(value) {
        Ok(buffer) => v8::Uint8Array::new(scope, buffer, 0, buffer.byte_length())?.into(),
        Err(_) => v8::Local::<v8::ArrayBufferView>::try_from(value).ok()?,
    };
    let mut out = vec![0u8; view.byte_length()];
    let copied = view.copy_contents(&mut out);
    (copied == out.len()).then_some(out)
}

fn bson_encode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    // A panic must not unwind into V8 (D15).
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bson_encode_inner(&mut *scope, &args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __bson_encode"),
        );
    }
}

fn bson_encode_inner<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(types) = Types::read(scope, args.get(1)) else {
        throw(
            scope,
            &OpError::type_error("__bson_encode needs the BSON types"),
        );
        return;
    };
    let value = args.get(0);
    let top = v8::Local::<v8::Object>::try_from(value)
        .ok()
        .filter(|o| !value.is_array() && !o.is_function() && !value.is_null());
    let Some(top) = top else {
        throw(
            scope,
            &OpError::type_error("BSON.encode takes an object or a Map: a document"),
        );
        return;
    };
    let mut encoder = Encoder {
        types: &types,
        w: Writer::new(),
        ancestors: Vec::new(),
    };
    let encoded = match top.get_prototype(scope) {
        Some(prototype) if prototype != types.object_prototype && !top.is_map() => {
            // A BSON value on its own is not a document.
            if is_bson_type(&types, prototype) {
                Err(Fail::Type(
                    "BSON.encode takes a document, not a single BSON value".into(),
                ))
            } else {
                encoder.document(scope, top)
            }
        }
        Some(_) => encoder.document(scope, top),
        None => Err(Fail::Thrown),
    };
    match encoded.and_then(|()| encoder.w.finish().map_err(Fail::from)) {
        Ok(bytes) => {
            let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
            let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
            if let Some(view) = v8::Uint8Array::new(scope, buffer, 0, buffer.byte_length()) {
                rv.set(view.into());
            }
        }
        Err(Fail::Thrown) => {}
        Err(Fail::Type(message)) => throw(scope, &OpError::type_error(message)),
        Err(Fail::Range(message)) => throw(scope, &OpError::range_error(message)),
    }
}

fn is_bson_type(t: &Types<'_>, prototype: v8::Local<'_, v8::Value>) -> bool {
    [
        t.object_id,
        t.decimal128,
        t.uuid,
        t.timestamp,
        t.binary,
        t.code,
        t.regexp,
        t.min_key,
        t.max_key,
        t.double,
    ]
    .contains(&prototype)
}

// ---- decode -----------------------------------------------------------------

struct Decoder<'t, 's> {
    types: &'t Types<'s>,
}

/// A decode failure: malformed bytes, or a V8 allocation that failed (which
/// leaves its own exception).
enum Bad {
    Bytes(es_runtime_bson::Error),
    Thrown,
}

impl From<es_runtime_bson::Error> for Bad {
    fn from(e: es_runtime_bson::Error) -> Self {
        Bad::Bytes(e)
    }
}

type Decoded<'s, T> = std::result::Result<v8::Local<'s, T>, Bad>;

impl<'s> Decoder<'_, 's> {
    fn document(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        doc: Document<'_>,
        depth: usize,
    ) -> Decoded<'s, v8::Object> {
        check_depth(depth)?;
        let mut names: Vec<v8::Local<v8::Name>> = Vec::new();
        let mut values: Vec<v8::Local<v8::Value>> = Vec::new();
        for element in doc.iter() {
            let (key, element) = element?;
            let name =
                v8::String::new_from_utf8(scope, key.as_bytes(), v8::NewStringType::Internalized)
                    .ok_or(Bad::Thrown)?;
            let value = self.element(scope, element, depth)?;
            names.push(name.into());
            values.push(value);
        }
        let t = self.types;
        fast_object(
            scope,
            t.object_prototype,
            t.object_prototype,
            &names,
            &values,
        )
    }

    fn array(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        doc: Document<'_>,
        depth: usize,
    ) -> Decoded<'s, v8::Array> {
        check_depth(depth)?;
        let mut values = Vec::new();
        // An array's keys are ignored: its values are taken in order.
        for element in doc.iter() {
            values.push(self.element(scope, element?.1, depth)?);
        }
        Ok(v8::Array::new_with_elements(scope, &values))
    }

    fn element(
        &self,
        scope: &mut v8::PinScope<'s, '_>,
        element: Element<'_>,
        depth: usize,
    ) -> Decoded<'s, v8::Value> {
        let t = self.types;
        Ok(match element {
            Element::Double(n) => v8::Number::new(scope, n).into(),
            Element::String(s) | Element::Symbol(s) => string(scope, s)?.into(),
            Element::Document(doc) => self.document(scope, doc, depth + 1)?.into(),
            Element::Array(doc) => self.array(scope, doc, depth + 1)?.into(),
            Element::Binary { subtype: 0, bytes } => uint8_array(scope, bytes)?.into(),
            Element::Binary { subtype: 4, bytes } if bytes.len() == 16 => {
                let hex = string(scope, &es_runtime_bson::to_hex(bytes))?;
                t.instance(scope, t.uuid, &[t.data.into()], &[hex.into()])?
                    .into()
            }
            Element::Binary { subtype, bytes } => {
                let buffer = uint8_array(scope, bytes)?;
                let sub_type = v8::Integer::new(scope, i32::from(subtype));
                t.instance(
                    scope,
                    t.binary,
                    &[t.names.buffer.into(), t.names.sub_type.into()],
                    &[buffer.into(), sub_type.into()],
                )?
                .into()
            }
            // Deprecated: read as null, its replacement. `undefined` would drop
            // the field on the way back.
            Element::Undefined => v8::null(scope).into(),
            Element::ObjectId(id) => self.object_id(scope, id)?.into(),
            Element::Boolean(b) => v8::Boolean::new(scope, b).into(),
            #[expect(
                clippy::cast_precision_loss,
                reason = "a Date holds whole milliseconds within ±8.64e15; past that it is an Invalid Date, as in the official driver"
            )]
            Element::DateTime(ms) => v8::Date::new(scope, ms as f64).ok_or(Bad::Thrown)?.into(),
            Element::Null => v8::null(scope).into(),
            Element::Regex { pattern, options } => {
                let pattern = string(scope, pattern)?;
                let options = string(scope, options)?;
                t.instance(
                    scope,
                    t.regexp,
                    &[t.names.pattern.into(), t.names.options.into()],
                    &[pattern.into(), options.into()],
                )?
                .into()
            }
            // Deprecated: read as the DBRef it was replaced by.
            Element::DbPointer { namespace, id } => {
                let namespace = string(scope, namespace)?;
                let id = self.object_id(scope, id)?;
                fast_object(
                    scope,
                    t.object_prototype,
                    t.object_prototype,
                    &[t.names.reference.into(), t.names.id.into()],
                    &[namespace.into(), id.into()],
                )?
                .into()
            }
            Element::Code(code) => {
                let code = string(scope, code)?;
                let null = v8::null(scope);
                t.instance(
                    scope,
                    t.code,
                    &[t.names.code.into(), t.names.scope.into()],
                    &[code.into(), null.into()],
                )?
                .into()
            }
            Element::CodeWithScope { code, scope: doc } => {
                let code = string(scope, code)?;
                let code_scope = self.document(scope, doc, depth + 1)?;
                t.instance(
                    scope,
                    t.code,
                    &[t.names.code.into(), t.names.scope.into()],
                    &[code.into(), code_scope.into()],
                )?
                .into()
            }
            Element::Int32(n) => v8::Integer::new(scope, n).into(),
            Element::Timestamp(n) => {
                let big = v8::BigInt::new_from_u64(scope, n);
                t.instance(scope, t.timestamp, &[t.data.into()], &[big.into()])?
                    .into()
            }
            Element::Int64(n) => v8::BigInt::new_from_i64(scope, n).into(),
            Element::Decimal128(bytes) => {
                let hex = string(scope, &es_runtime_bson::to_hex(&bytes))?;
                t.instance(scope, t.decimal128, &[t.data.into()], &[hex.into()])?
                    .into()
            }
            Element::MinKey => t.instance(scope, t.min_key, &[], &[])?.into(),
            Element::MaxKey => t.instance(scope, t.max_key, &[], &[])?.into(),
        })
    }

    fn object_id(&self, scope: &mut v8::PinScope<'s, '_>, id: [u8; 12]) -> Decoded<'s, v8::Object> {
        let t = self.types;
        let hex = string(scope, &es_runtime_bson::to_hex(&id))?;
        t.instance(scope, t.object_id, &[t.data.into()], &[hex.into()])
    }
}

fn check_depth(depth: usize) -> std::result::Result<(), Bad> {
    if depth > MAX_DEPTH {
        return Err(Bad::Bytes(es_runtime_bson::Error::new(format!(
            "documents nest more than {MAX_DEPTH} deep"
        ))));
    }
    Ok(())
}

fn string<'s>(scope: &mut v8::PinScope<'s, '_>, text: &str) -> Decoded<'s, v8::String> {
    // ASCII is Latin-1, so V8 takes it as a one-byte string with no decoding.
    let made = if text.is_ascii() {
        v8::String::new_from_one_byte(scope, text.as_bytes(), v8::NewStringType::Normal)
    } else {
        v8::String::new_from_utf8(scope, text.as_bytes(), v8::NewStringType::Normal)
    };
    made.ok_or(Bad::Thrown)
}

fn uint8_array<'s>(scope: &mut v8::PinScope<'s, '_>, bytes: &[u8]) -> Decoded<'s, v8::Uint8Array> {
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes.to_vec()).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    v8::Uint8Array::new(scope, buffer, 0, bytes.len()).ok_or(Bad::Thrown)
}

fn bson_decode<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: v8::FunctionCallbackArguments<'s>,
    rv: v8::ReturnValue<'s, v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bson_decode_inner(&mut *scope, &args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __bson_decode"),
        );
    }
}

fn bson_decode_inner<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    args: &v8::FunctionCallbackArguments<'s>,
    mut rv: v8::ReturnValue<'s, v8::Value>,
) {
    let Some(types) = Types::read(scope, args.get(1)) else {
        throw(
            scope,
            &OpError::type_error("__bson_decode needs the BSON types"),
        );
        return;
    };
    let input = args.get(0);
    let Some(bytes) = (input.is_array_buffer_view() || input.is_array_buffer())
        .then(|| bytes_of(scope, input))
        .flatten()
    else {
        throw(
            scope,
            &OpError::type_error("BSON.decode takes a Uint8Array or an ArrayBuffer"),
        );
        return;
    };
    let decoded = Document::new(&bytes)
        .map_err(Bad::from)
        .and_then(|doc| Decoder { types: &types }.document(scope, doc, 1));
    match decoded {
        Ok(object) => rv.set(object.into()),
        Err(Bad::Thrown) => {}
        Err(Bad::Bytes(e)) => throw(
            scope,
            &OpError::new(ExceptionClass::SyntaxError, format!("Invalid BSON: {e}")),
        ),
    }
}

/// An object with `prototype` and these properties, built so V8 keeps it in
/// fast mode.
///
/// `Object::with_prototype_and_properties` would do this in one call, but V8
/// makes that object a dictionary, and every later property read on it is
/// several times slower than on what `JSON.parse` builds. An object made empty
/// and given its properties one at a time takes the same shape transitions as
/// any other with those keys, so a thousand documents with the same fields
/// share one shape. The properties are defined, not assigned: a `__proto__`
/// key becomes an own property, and a repeated key keeps its first place and
/// its last value.
fn fast_object<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    object_prototype: v8::Local<'s, v8::Value>,
    prototype: v8::Local<'s, v8::Value>,
    names: &[v8::Local<'s, v8::Name>],
    values: &[v8::Local<'s, v8::Value>],
) -> Decoded<'s, v8::Object> {
    let object = v8::Object::new(scope);
    if prototype != object_prototype {
        object.set_prototype(scope, prototype).ok_or(Bad::Thrown)?;
    }
    for (name, value) in names.iter().zip(values) {
        object
            .create_data_property(scope, *name, *value)
            .ok_or(Bad::Thrown)?;
    }
    Ok(object)
}
