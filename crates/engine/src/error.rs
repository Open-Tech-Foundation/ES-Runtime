//! Engine-layer error (DECISIONS.md D12).
//!
//! One typed enum for this layer, mapped to a JS exception class via
//! [`IntoException`]. Engine errors either wrap a `common` error or describe a
//! V8-side failure (compilation, an uncaught JS exception, or an engine
//! invariant violation).

use es_runtime_common::{ExceptionClass, IntoException};

/// An error from the engine layer.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A lower-layer (`common`) error — e.g. invalid limits or a capability
    /// denial — surfaced through the engine. Mapping is delegated to it.
    #[error(transparent)]
    Common(#[from] es_runtime_common::Error),

    /// Source failed to compile. Surfaces as a `SyntaxError`.
    #[error("script compilation failed: {message}")]
    Compile {
        /// The V8 diagnostic for the failure.
        message: String,
    },

    /// Execution threw an uncaught JS exception. Phase 1 carries the exception's
    /// stringified message; preserving its original JS class through the
    /// boundary is a Phase 2 refinement (recorded in DECISIONS.md D3a).
    #[error("uncaught exception: {message}")]
    Execution {
        /// The stringified uncaught exception.
        message: String,
    },

    /// An engine invariant did not hold (V8 returned `None` where success was
    /// required, an over-long string, etc.). Indicates a bug or a resource
    /// boundary, not adversarial JS.
    #[error("engine internal error: {0}")]
    Internal(String),

    /// Execution was terminated before completing — by the watchdog
    /// (`InterruptHandle::terminate`) or the near-heap-limit guard. The script
    /// is stopped cleanly, never an OOM or a hang (ARCHITECTURE.md §7, SPEC §4).
    /// The engine should be considered spent; the embedder discards it.
    #[error("execution terminated: {reason}")]
    Terminated {
        /// Why execution was stopped (e.g. "heap limit exceeded", "timed out").
        reason: String,
    },
}

impl IntoException for Error {
    fn exception_class(&self) -> ExceptionClass {
        match self {
            Error::Common(e) => e.exception_class(),
            Error::Compile { .. } => ExceptionClass::SyntaxError,
            // A re-thrown uncaught exception is, lacking class preservation,
            // surfaced as a generic Error for now (see D3a).
            Error::Execution { .. } => ExceptionClass::Error,
            Error::Internal(_) => ExceptionClass::Error,
            Error::Terminated { .. } => ExceptionClass::Error,
        }
    }
}

/// Engine result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_maps_to_syntax_error() {
        let err = Error::Compile {
            message: "Unexpected token".into(),
        };
        assert_eq!(err.exception_class(), ExceptionClass::SyntaxError);
    }

    #[test]
    fn common_error_mapping_is_delegated() {
        let err: Error = es_runtime_common::Error::Config("bad".into()).into();
        assert_eq!(err.exception_class(), ExceptionClass::TypeError);
    }

    #[test]
    fn execution_maps_to_generic_error() {
        let err = Error::Execution {
            message: "boom".into(),
        };
        assert_eq!(err.exception_class(), ExceptionClass::Error);
    }
}
