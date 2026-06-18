//! Marshaled JS values crossing engine → caller.
//!
//! The public result type of [`Engine::eval`](crate::Engine::eval). Kept free of
//! any `v8` type so the engine boundary holds (ARCHITECTURE.md §3): callers see
//! only plain Rust. Phase 1 marshals the JS primitives needed to prove the
//! pipeline; structural marshaling of objects, arrays, typed arrays, promises,
//! and functions arrives with the op system and zero-copy work (Phases 2/8).

/// A JS value marshaled into Rust.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// JS `undefined`.
    Undefined,
    /// JS `null`.
    Null,
    /// A JS boolean.
    Bool(bool),
    /// A JS number (IEEE-754 double; integers included).
    Number(f64),
    /// A JS string, decoded to a Rust `String`.
    String(String),
    /// Bytes from a JS `Uint8Array`/typed array view (copied). Marshals back to
    /// a `Uint8Array`. This is the interim copying path; true zero-copy
    /// `ArrayBuffer` transfer is Phase 8 (ARCHITECTURE.md §9).
    Bytes(Vec<u8>),
    /// An ordered sequence, marshaled to/from a JS array. JS arrays crossing
    /// **into** Rust marshal structurally too (each element recursively), so this
    /// round-trips — host ops both return it (e.g. the URL ops' href+offsets) and
    /// receive it (resolved D3a).
    Array(Vec<Value>),
    /// A structured JS object as ordered `(key, value)` pairs, marshaled to/from a
    /// JS object (own enumerable string keys; recursive values).
    Object(Vec<(String, Value)>),
    /// A JS value not yet marshaled structurally. Carries the value's
    /// `String(value)` coercion so it is still inspectable; later phases replace
    /// this with structured variants.
    Other(String),
}

impl Value {
    /// Returns the number if this is [`Value::Number`], else `None`. Convenience
    /// for tests and callers expecting a numeric result.
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// Returns the string slice if this is [`Value::String`], else `None`.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the bytes if this is [`Value::Bytes`], else `None`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    /// Consumes the value into an owned byte buffer when it carries bytes —
    /// moving a [`Value::Bytes`] `Vec` out without copying, or taking a
    /// [`Value::String`]'s already-UTF-8 bytes. Returns `None` for other
    /// variants. Lets byte sinks (e.g. file writes) avoid re-copying what
    /// marshaling already produced.
    pub fn into_bytes(self) -> Option<Vec<u8>> {
        match self {
            Value::Bytes(b) => Some(b),
            Value::String(s) => Some(s.into_bytes()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessors_match_variant() {
        assert_eq!(Value::Number(2.0).as_number(), Some(2.0));
        assert_eq!(Value::Number(2.0).as_str(), None);
        assert_eq!(Value::String("hi".into()).as_str(), Some("hi"));
        assert_eq!(Value::Bool(true).as_number(), None);
    }
}
