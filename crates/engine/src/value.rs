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
    /// A JS value Phase 1 does not yet marshal structurally. Carries the value's
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
