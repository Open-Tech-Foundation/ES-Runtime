//! The error model (DECISIONS.md D12, ARCHITECTURE.md §8).
//!
//! The rule is *one typed error enum per layer*, each mapping cleanly to a JS
//! exception class and never silently swallowed. This module provides the two
//! shared pieces that make that uniform:
//!
//! - [`ExceptionClass`] — the canonical set of JS-visible exception kinds an
//!   error can surface as.
//! - [`IntoException`] — the trait every layer's error enum implements so the
//!   `engine` boundary can convert *any* Rust error into the right JS exception
//!   at a single, well-typed seam.
//!
//! [`Error`] is `common`'s own layer error (capability denials, configuration
//! faults); crates above define their own and implement [`IntoException`] the
//! same way.

/// A JS-visible exception class.
///
/// These are the constructors a thrown value is built from when a Rust error
/// crosses back into JS. The standard `Error` subclasses are modeled directly;
/// WebIDL [`DomException`](ExceptionClass::DomException) is modeled as its
/// `.name` string, since that is exactly what distinguishes one `DOMException`
/// from another in the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExceptionClass {
    /// `Error` — the generic base class.
    Error,
    /// `RangeError` — a value outside the permitted range (e.g. a bad length).
    RangeError,
    /// `ReferenceError`.
    ReferenceError,
    /// `SyntaxError` — including JS parse failures surfaced from the engine.
    SyntaxError,
    /// `TypeError` — wrong type, or a contract violation at the JS↔Rust boundary.
    TypeError,
    /// `URIError`.
    UriError,
    /// A WebIDL `DOMException`, identified by its `.name` (e.g. `"AbortError"`,
    /// `"NotAllowedError"`). The name is the platform's discriminator and is a
    /// `DOMString`, so it is carried as a static string rather than re-encoded
    /// as a second enum.
    DomException(&'static str),
}

impl ExceptionClass {
    /// The `DOMException` name a permission/capability denial surfaces as.
    ///
    /// Web specifications raise `"NotAllowedError"` when an operation is blocked
    /// by the permission model; capability denials reuse it (DECISIONS.md D7).
    pub const NOT_ALLOWED: ExceptionClass = ExceptionClass::DomException("NotAllowedError");

    /// The constructor name as it appears in JS (`"TypeError"`, `"DOMException"`,
    /// …). For a [`DomException`](ExceptionClass::DomException) this is the class
    /// name `"DOMException"`, not the instance's `.name`; use
    /// [`dom_exception_name`](Self::dom_exception_name) for the latter.
    pub const fn js_name(self) -> &'static str {
        match self {
            ExceptionClass::Error => "Error",
            ExceptionClass::RangeError => "RangeError",
            ExceptionClass::ReferenceError => "ReferenceError",
            ExceptionClass::SyntaxError => "SyntaxError",
            ExceptionClass::TypeError => "TypeError",
            ExceptionClass::UriError => "URIError",
            ExceptionClass::DomException(_) => "DOMException",
        }
    }

    /// The `DOMException` `.name`, or `None` for the plain `Error` subclasses.
    pub const fn dom_exception_name(self) -> Option<&'static str> {
        match self {
            ExceptionClass::DomException(name) => Some(name),
            _ => None,
        }
    }
}

/// A stable, guest-facing error code (SPEC §6 Phase 13).
///
/// Exception **messages** are human prose and may be reworded at any time; the
/// **code** is the contract guest code may branch on. It surfaces to JS as a
/// `code` string property on the thrown exception (e.g. `"ERR_NOT_FOUND"`).
/// The set may grow, and an error without a stable classification simply
/// carries no `code` — so guests test `e.code === "ERR_X"`, never exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    /// A required capability was not granted (deny-by-default, D7).
    CapabilityDenied,
    /// The backing provider for this API is not installed on the runtime.
    ProviderUnavailable,
    /// The operation was cancelled before completing.
    Cancelled,
    /// The entropy source failed to produce randomness.
    Entropy,
    /// A path does not exist.
    NotFound,
    /// The target already exists.
    AlreadyExists,
    /// The OS denied access (distinct from a runtime capability denial).
    PermissionDenied,
    /// A file operation hit a directory.
    IsDirectory,
    /// A directory operation hit a non-directory.
    NotDirectory,
    /// The directory is not empty.
    DirectoryNotEmpty,
    /// A real (canonicalized) path escapes the filesystem root jail (D25).
    JailEscape,
    /// The peer refused the connection.
    ConnectionRefused,
    /// The connection was reset or aborted by the peer.
    ConnectionReset,
    /// The operation timed out.
    TimedOut,
    /// The local address is already in use.
    AddressInUse,
    /// The host or network is unreachable.
    Unreachable,
    /// Name resolution failed.
    Dns,
    /// TLS handshake or certificate verification failed.
    Tls,
    /// An I/O failure with no finer stable classification.
    Io,
}

impl ErrorCode {
    /// The stable string form surfaced to JS as the exception's `code`.
    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::CapabilityDenied => "ERR_CAPABILITY_DENIED",
            ErrorCode::ProviderUnavailable => "ERR_PROVIDER_UNAVAILABLE",
            ErrorCode::Cancelled => "ERR_CANCELLED",
            ErrorCode::Entropy => "ERR_ENTROPY",
            ErrorCode::NotFound => "ERR_NOT_FOUND",
            ErrorCode::AlreadyExists => "ERR_ALREADY_EXISTS",
            ErrorCode::PermissionDenied => "ERR_PERMISSION_DENIED",
            ErrorCode::IsDirectory => "ERR_IS_DIRECTORY",
            ErrorCode::NotDirectory => "ERR_NOT_DIRECTORY",
            ErrorCode::DirectoryNotEmpty => "ERR_DIRECTORY_NOT_EMPTY",
            ErrorCode::JailEscape => "ERR_JAIL_ESCAPE",
            ErrorCode::ConnectionRefused => "ERR_CONNECTION_REFUSED",
            ErrorCode::ConnectionReset => "ERR_CONNECTION_RESET",
            ErrorCode::TimedOut => "ERR_TIMED_OUT",
            ErrorCode::AddressInUse => "ERR_ADDRESS_IN_USE",
            ErrorCode::Unreachable => "ERR_UNREACHABLE",
            ErrorCode::Dns => "ERR_DNS",
            ErrorCode::Tls => "ERR_TLS",
            ErrorCode::Io => "ERR_IO",
        }
    }

    /// The stable classification of an [`std::io::ErrorKind`], if it has one.
    /// Unstable/unclassified kinds map to the generic [`ErrorCode::Io`].
    pub fn from_io_kind(kind: std::io::ErrorKind) -> ErrorCode {
        use std::io::ErrorKind as K;
        match kind {
            K::NotFound => ErrorCode::NotFound,
            K::AlreadyExists => ErrorCode::AlreadyExists,
            K::PermissionDenied => ErrorCode::PermissionDenied,
            K::IsADirectory => ErrorCode::IsDirectory,
            K::NotADirectory => ErrorCode::NotDirectory,
            K::DirectoryNotEmpty => ErrorCode::DirectoryNotEmpty,
            K::ConnectionRefused => ErrorCode::ConnectionRefused,
            K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe => ErrorCode::ConnectionReset,
            K::TimedOut => ErrorCode::TimedOut,
            K::AddrInUse => ErrorCode::AddressInUse,
            K::HostUnreachable | K::NetworkUnreachable | K::NetworkDown => ErrorCode::Unreachable,
            K::Interrupted => ErrorCode::Cancelled,
            _ => ErrorCode::Io,
        }
    }
}

/// Conversion of a layer error into the JS exception it should surface as.
///
/// Implemented by every layer's error enum so the engine boundary has a single
/// generic point at which a Rust error becomes a JS exception (DECISIONS.md
/// D12). Implementors return the [`ExceptionClass`]; the message defaults to the
/// value's [`Display`](std::fmt::Display) output.
pub trait IntoException: std::fmt::Display {
    /// The JS exception class this error surfaces as.
    fn exception_class(&self) -> ExceptionClass;

    /// The text for the JS exception's `.message`. Defaults to `Display`.
    fn exception_message(&self) -> String {
        self.to_string()
    }

    /// The stable guest-facing `code` set on the JS exception, if this error
    /// has a stable classification (SPEC §6 Phase 13). Defaults to none.
    fn exception_code(&self) -> Option<ErrorCode> {
        None
    }
}

/// `common`-layer error.
///
/// Per DECISIONS.md D12 this is one enum for one layer. It is `#[non_exhaustive]`
/// so variants can be added without breaking downstream `match`es.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A capability required for an operation was not granted (DECISIONS.md D7).
    #[error("capability denied: {0:?}")]
    CapabilityDenied(crate::capability::Capability),

    /// A configuration value was invalid or inconsistent.
    #[error("invalid configuration: {0}")]
    Config(String),
}

impl IntoException for Error {
    fn exception_class(&self) -> ExceptionClass {
        match self {
            // A denied capability is a permission failure → NotAllowedError.
            Error::CapabilityDenied(_) => ExceptionClass::NOT_ALLOWED,
            // A bad configuration is a caller contract violation → TypeError.
            Error::Config(_) => ExceptionClass::TypeError,
        }
    }

    fn exception_code(&self) -> Option<ErrorCode> {
        match self {
            Error::CapabilityDenied(_) => Some(ErrorCode::CapabilityDenied),
            Error::Config(_) => None,
        }
    }
}

/// `common`'s result alias. Crates above define their own over their own error.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    #[test]
    fn js_name_matches_constructor() {
        assert_eq!(ExceptionClass::TypeError.js_name(), "TypeError");
        assert_eq!(ExceptionClass::UriError.js_name(), "URIError");
        assert_eq!(ExceptionClass::NOT_ALLOWED.js_name(), "DOMException");
    }

    #[test]
    fn dom_exception_name_only_for_dom() {
        assert_eq!(
            ExceptionClass::NOT_ALLOWED.dom_exception_name(),
            Some("NotAllowedError")
        );
        assert_eq!(ExceptionClass::TypeError.dom_exception_name(), None);
    }

    #[test]
    fn error_codes_have_stable_err_prefixed_strings() {
        assert_eq!(
            ErrorCode::CapabilityDenied.as_str(),
            "ERR_CAPABILITY_DENIED"
        );
        assert_eq!(ErrorCode::NotFound.as_str(), "ERR_NOT_FOUND");
        assert_eq!(ErrorCode::JailEscape.as_str(), "ERR_JAIL_ESCAPE");
        assert_eq!(ErrorCode::Tls.as_str(), "ERR_TLS");
    }

    #[test]
    fn io_kinds_classify_with_a_generic_fallback() {
        use std::io::ErrorKind as K;
        assert_eq!(ErrorCode::from_io_kind(K::NotFound), ErrorCode::NotFound);
        assert_eq!(
            ErrorCode::from_io_kind(K::AddrInUse),
            ErrorCode::AddressInUse
        );
        assert_eq!(
            ErrorCode::from_io_kind(K::ConnectionAborted),
            ErrorCode::ConnectionReset
        );
        assert_eq!(ErrorCode::from_io_kind(K::Other), ErrorCode::Io);
    }

    #[test]
    fn capability_denial_carries_its_code() {
        assert_eq!(
            Error::CapabilityDenied(Capability::Net).exception_code(),
            Some(ErrorCode::CapabilityDenied)
        );
        assert_eq!(Error::Config("x".into()).exception_code(), None);
    }

    #[test]
    fn capability_denial_maps_to_not_allowed() {
        let err = Error::CapabilityDenied(Capability::Net);
        assert_eq!(err.exception_class(), ExceptionClass::NOT_ALLOWED);
        assert!(err.exception_message().contains("Net"));
    }

    #[test]
    fn config_error_maps_to_type_error() {
        let err = Error::Config("heap_limit_bytes must be non-zero".into());
        assert_eq!(err.exception_class(), ExceptionClass::TypeError);
        assert_eq!(err.exception_message(), err.to_string());
    }
}
