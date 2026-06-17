//! Conversions across the V8 boundary.
//!
//! Centralizes both directions: marshaling V8 values into a [`Value`] and back,
//! and building/throwing JS exceptions from any [`IntoException`]. Keeping this
//! in one module means evaluation, the op dispatcher, and snapshot building
//! share one implementation rather than each re-deriving the scope plumbing.

use es_runtime_common::{ExceptionClass, IntoException};

use crate::value::Value;

/// Marshals a V8 value into a [`Value`].
///
/// Phase 1/2 handle the JS primitives; any other value is coerced to its
/// `String(value)` form as [`Value::Other`] (see the D3a leak note in
/// DECISIONS.md — structured marshaling is later).
pub(crate) fn marshal(scope: &v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> Value {
    if value.is_undefined() {
        Value::Undefined
    } else if value.is_null() {
        Value::Null
    } else if value.is_boolean() {
        Value::Bool(value.boolean_value(scope))
    } else if value.is_number() {
        Value::Number(value.number_value(scope).unwrap_or(f64::NAN))
    } else if value.is_string() {
        Value::String(js_to_string(scope, value))
    } else if value.is_array_buffer_view() {
        // Uint8Array and other typed-array/DataView views — copied out
        // (interim; zero-copy is Phase 8). Bare ArrayBuffers are wrapped as a
        // Uint8Array in the prelude before crossing, so a view suffices here.
        let view = v8::Local::<v8::ArrayBufferView>::try_from(value).expect("checked view");
        let mut buf = vec![0u8; view.byte_length()];
        view.copy_contents(&mut buf);
        Value::Bytes(buf)
    } else if value.is_array() {
        let array = v8::Local::<v8::Array>::try_from(value).expect("checked array");
        let len = array.length();
        let mut items = Vec::with_capacity(len as usize);
        for i in 0..len {
            let item = array.get_index(scope, i).unwrap_or_else(|| v8::undefined(scope).into());
            items.push(marshal(scope, item));
        }
        Value::Array(items)
    } else if value.is_object() && !value.is_function() && !value.is_promise() {
        let obj = v8::Local::<v8::Object>::try_from(value).expect("checked object");
        let prop_names = obj.get_own_property_names(scope, v8::GetPropertyNamesArgs::default()).unwrap_or_else(|| v8::Array::new(scope, 0));
        let len = prop_names.length();
        let mut map = Vec::with_capacity(len as usize);
        for i in 0..len {
            let key = prop_names.get_index(scope, i).unwrap_or_else(|| v8::undefined(scope).into());
            let val = obj.get(scope, key).unwrap_or_else(|| v8::undefined(scope).into());
            map.push((js_to_string(scope, key), marshal(scope, val)));
        }
        Value::Object(map)
    } else {
        Value::Other(js_to_string(scope, value))
    }
}

/// Marshals a [`Value`] into a V8 value for return to JS. Consumes the value so
/// owned byte buffers move into the `ArrayBuffer` backing store without a copy.
pub(crate) fn value_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: Value,
) -> v8::Local<'s, v8::Value> {
    match value {
        Value::Undefined => v8::undefined(scope).into(),
        Value::Null => v8::null(scope).into(),
        Value::Bool(b) => v8::Boolean::new(scope, b).into(),
        Value::Number(n) => v8::Number::new(scope, n).into(),
        Value::String(s) | Value::Other(s) => js_string(scope, &s).into(),
        Value::Bytes(bytes) => bytes_to_uint8array(scope, bytes).into(),
        Value::Array(items) => {
            let elements: Vec<v8::Local<v8::Value>> = items
                .into_iter()
                .map(|item| value_to_js(scope, item))
                .collect();
            v8::Array::new_with_elements(scope, &elements).into()
        }
        Value::Object(map) => {
            let obj = v8::Object::new(scope);
            for (key, val) in map {
                let v8_key = v8::String::new(scope, &key).unwrap_or_else(|| v8::String::empty(scope));
                let v8_val = value_to_js(scope, val);
                obj.set(scope, v8_key.into(), v8_val);
            }
            obj.into()
        }
    }
}

/// Builds a `Uint8Array` whose `ArrayBuffer` takes ownership of `bytes` — no
/// copy; the Vec becomes the backing store.
fn bytes_to_uint8array<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> v8::Local<'s, v8::Uint8Array> {
    let len = bytes.len();
    let store = v8::ArrayBuffer::new_backing_store_from_vec(bytes).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    v8::Uint8Array::new(scope, buffer, 0, len).expect("uint8array construction")
}

/// Builds (without throwing) the JS exception a layer error should surface as
/// (DECISIONS.md D12).
///
/// `DOMException` has no constructor in bare V8 — it is a web-platform class that
/// only exists once the runtime prelude defines it (a later phase). Until then a
/// `DOMException`-classed error is surfaced as a plain `Error` whose message is
/// prefixed with the `.name` (recorded as a D3a leak note).
pub(crate) fn build_exception<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    err: &dyn IntoException,
) -> v8::Local<'s, v8::Value> {
    let class = err.exception_class();

    // Fallback/dynamic constructor lookup for classes V8 doesn't provide natively.
    let try_construct = |scope: &mut v8::PinScope<'s, '_>,
                         class_name: &str,
                         args: &[v8::Local<'s, v8::Value>]|
     -> Option<v8::Local<'s, v8::Value>> {
        let context = scope.get_current_context();
        let global = context.global(scope);
        let key = v8::String::new(scope, class_name)?;
        let constructor = global.get(scope, key.into())?;
        if constructor.is_function() {
            let constructor = v8::Local::<v8::Function>::try_from(constructor).ok()?;
            let exception = constructor.new_instance(scope, args)?;
            Some(exception.into())
        } else {
            None
        }
    };

    if let ExceptionClass::DomException(name) = class {
        if let Some(msg_val) = v8::String::new(scope, &err.exception_message())
            && let Some(name_val) = v8::String::new(scope, name)
            && let Some(ex) =
                try_construct(scope, "DOMException", &[msg_val.into(), name_val.into()])
        {
            return ex;
        }
    } else if let ExceptionClass::UriError = class
        && let Some(msg_val) = v8::String::new(scope, &err.exception_message())
        && let Some(ex) = try_construct(scope, "URIError", &[msg_val.into()])
    {
        return ex;
    }

    let text = match class.dom_exception_name() {
        Some(name) => format!("{name}: {}", err.exception_message()),
        None => err.exception_message(),
    };
    let message = js_string(scope, &text);
    match class {
        ExceptionClass::RangeError => v8::Exception::range_error(scope, message),
        ExceptionClass::ReferenceError => v8::Exception::reference_error(scope, message),
        ExceptionClass::SyntaxError => v8::Exception::syntax_error(scope, message),
        ExceptionClass::TypeError => v8::Exception::type_error(scope, message),
        // `Error` and any future class: default to a plain `Error`.
        _ => v8::Exception::error(scope, message),
    }
}

/// Builds the JS exception and throws it into `scope`, so a host-side error
/// becomes a thrown JS value rather than a Rust unwind (ARCHITECTURE.md §7).
pub(crate) fn throw(scope: &mut v8::PinScope<'_, '_>, err: &dyn IntoException) {
    let exception = build_exception(scope, err);
    scope.throw_exception(exception);
}

/// Renders the exception currently held by `scope` to a message, falling back to
/// `fallback` when none is present or it cannot be stringified.
pub(crate) fn describe_exception(
    scope: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    fallback: &str,
) -> String {
    let exception = match scope.exception() {
        Some(e) => e,
        None => return fallback.to_string(),
    };

    if let Some(stack) = exception_stack(scope, exception) {
        return stack;
    }

    if let Some(msg) = scope.message() {
        let text = msg.get(scope).to_rust_string_lossy(scope);
        // Fallback to building a minimal trace from v8::Message if .stack is missing
        if let Some(trace) = msg.get_stack_trace(scope)
            && trace.get_frame_count() > 0
            && let Some(frame) = trace.get_frame(scope, 0)
        {
            let file = frame
                .get_script_name_or_source_url(scope)
                .map(|s| s.to_rust_string_lossy(scope))
                .unwrap_or_else(|| "<unknown>".to_string());
            let line = frame.get_line_number();
            let col = frame.get_column();
            return format!("{text}\n    at {file}:{line}:{col}");
        }
        return text;
    }

    js_to_string(scope, exception)
}

/// Returns the `.stack` string of an exception value, if it is an object with a
/// string `stack` property (i.e. an `Error`). Returns `None` otherwise.
fn exception_stack(
    scope: &mut v8::PinScope<'_, '_>,
    exception: v8::Local<v8::Value>,
) -> Option<String> {
    let obj = v8::Local::<v8::Object>::try_from(exception).ok()?;
    let key = v8::String::new(scope, "stack")?;
    let stack_val = obj.get(scope, key.into())?;
    stack_val
        .is_string()
        .then(|| stack_val.to_rust_string_lossy(scope))
}

/// Formats an exception value (e.g. from an unhandled promise rejection) by
/// extracting its `.stack` property if available, otherwise stringifying it.
pub(crate) fn format_exception(
    scope: &mut v8::PinScope<'_, '_>,
    exception: v8::Local<v8::Value>,
) -> String {
    exception_stack(scope, exception).unwrap_or_else(|| js_to_string(scope, exception))
}

/// Coerces any V8 value to a Rust `String` via JS `String(value)` semantics.
pub(crate) fn js_to_string(scope: &v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> String {
    value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
}

/// Builds a V8 string, falling back to an empty string if `s` exceeds V8's
/// maximum string length (vanishingly rare; never worth panicking over here).
fn js_string<'s>(scope: &mut v8::PinScope<'s, '_>, s: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, s).unwrap_or_else(|| v8::String::empty(scope))
}
