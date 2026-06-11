//! Conversions across the V8 boundary.
//!
//! Centralizes the two directions Phase 1 needs — marshaling a V8 value into a
//! [`Value`], and rendering a caught exception to a message — so evaluation
//! ([`engine`](crate::engine)) and snapshot building ([`snapshot`](crate::snapshot))
//! share one implementation rather than each re-deriving the scope plumbing.

use crate::value::Value;

/// Marshals a V8 value into a [`Value`].
///
/// Phase 1 handles the JS primitives; any other value is coerced to its
/// `String(value)` form as [`Value::Other`] (see the D3a leak note in
/// DECISIONS.md — structured marshaling lands with the op system).
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
        Value::String(stringify(scope, value))
    } else {
        Value::Other(stringify(scope, value))
    }
}

/// Renders the exception currently held by `scope` to a message, falling back to
/// `fallback` when none is present or it cannot be stringified.
pub(crate) fn describe_exception(
    scope: &mut v8::PinnedRef<'_, v8::TryCatch<v8::HandleScope>>,
    fallback: &str,
) -> String {
    match scope.exception() {
        Some(exception) => stringify(scope, exception),
        None => fallback.to_string(),
    }
}

/// Coerces any V8 value to a Rust `String` via JS `String(value)` semantics.
fn stringify(scope: &v8::PinScope<'_, '_>, value: v8::Local<v8::Value>) -> String {
    value
        .to_string(scope)
        .map(|s| s.to_rust_string_lossy(scope))
        .unwrap_or_default()
}
