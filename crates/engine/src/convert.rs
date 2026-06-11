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
    } else {
        Value::Other(js_to_string(scope, value))
    }
}

/// Marshals a [`Value`] into a V8 value for return to JS.
pub(crate) fn value_to_js<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: &Value,
) -> v8::Local<'s, v8::Value> {
    match value {
        Value::Undefined => v8::undefined(scope).into(),
        Value::Null => v8::null(scope).into(),
        Value::Bool(b) => v8::Boolean::new(scope, *b).into(),
        Value::Number(n) => v8::Number::new(scope, *n).into(),
        Value::String(s) | Value::Other(s) => js_string(scope, s).into(),
    }
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
        // `Error`, plus `URIError`/`DOMException` (no bare-V8 constructor) and
        // any future class: default to a plain `Error`.
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
    match scope.exception() {
        Some(exception) => js_to_string(scope, exception),
        None => fallback.to_string(),
    }
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
