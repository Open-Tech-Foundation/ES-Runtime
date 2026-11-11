//! HTTP/2 (RFC 9113), sans-I/O. So far, frames: [`frame`] reads and writes
//! them; the connection and stream state machine builds on it.

pub mod frame;

#[cfg(test)]
mod tests;

/// HTTP/2's error codes (RFC 9113 §7). An unknown code is kept as it came:
/// the RFC asks for it to be treated as `INTERNAL_ERROR`, not rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// `NO_ERROR` (0x0): not an error — a graceful shutdown.
    NoError,
    /// `PROTOCOL_ERROR` (0x1): a protocol rule was broken.
    ProtocolError,
    /// `INTERNAL_ERROR` (0x2): the endpoint failed on its own.
    InternalError,
    /// `FLOW_CONTROL_ERROR` (0x3): flow control was violated.
    FlowControlError,
    /// `SETTINGS_TIMEOUT` (0x4): a SETTINGS frame went unacknowledged.
    SettingsTimeout,
    /// `STREAM_CLOSED` (0x5): a frame arrived on a half-closed stream.
    StreamClosed,
    /// `FRAME_SIZE_ERROR` (0x6): a frame had the wrong size.
    FrameSizeError,
    /// `REFUSED_STREAM` (0x7): the stream was refused before any work.
    RefusedStream,
    /// `CANCEL` (0x8): the stream is no longer needed.
    Cancel,
    /// `COMPRESSION_ERROR` (0x9): the HPACK state cannot be kept in step.
    CompressionError,
    /// `CONNECT_ERROR` (0xa): a CONNECT tunnel failed.
    ConnectError,
    /// `ENHANCE_YOUR_CALM` (0xb): the peer is generating excessive load.
    EnhanceYourCalm,
    /// `INADEQUATE_SECURITY` (0xc): the transport's security is too weak.
    InadequateSecurity,
    /// `HTTP_1_1_REQUIRED` (0xd): use HTTP/1.1 for this request.
    Http11Required,
    /// A code this implementation does not know.
    Unknown(u32),
}

impl From<u32> for ErrorCode {
    fn from(code: u32) -> Self {
        match code {
            0x0 => ErrorCode::NoError,
            0x1 => ErrorCode::ProtocolError,
            0x2 => ErrorCode::InternalError,
            0x3 => ErrorCode::FlowControlError,
            0x4 => ErrorCode::SettingsTimeout,
            0x5 => ErrorCode::StreamClosed,
            0x6 => ErrorCode::FrameSizeError,
            0x7 => ErrorCode::RefusedStream,
            0x8 => ErrorCode::Cancel,
            0x9 => ErrorCode::CompressionError,
            0xa => ErrorCode::ConnectError,
            0xb => ErrorCode::EnhanceYourCalm,
            0xc => ErrorCode::InadequateSecurity,
            0xd => ErrorCode::Http11Required,
            other => ErrorCode::Unknown(other),
        }
    }
}

impl From<ErrorCode> for u32 {
    fn from(code: ErrorCode) -> Self {
        match code {
            ErrorCode::NoError => 0x0,
            ErrorCode::ProtocolError => 0x1,
            ErrorCode::InternalError => 0x2,
            ErrorCode::FlowControlError => 0x3,
            ErrorCode::SettingsTimeout => 0x4,
            ErrorCode::StreamClosed => 0x5,
            ErrorCode::FrameSizeError => 0x6,
            ErrorCode::RefusedStream => 0x7,
            ErrorCode::Cancel => 0x8,
            ErrorCode::CompressionError => 0x9,
            ErrorCode::ConnectError => 0xa,
            ErrorCode::EnhanceYourCalm => 0xb,
            ErrorCode::InadequateSecurity => 0xc,
            ErrorCode::Http11Required => 0xd,
            ErrorCode::Unknown(other) => other,
        }
    }
}

/// An HTTP/2 error and its reach (RFC 9113 §5.4): a stream error resets one
/// stream with RST_STREAM; a connection error ends the connection with GOAWAY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2Error {
    /// Ends the whole connection.
    Connection {
        /// The code sent with the GOAWAY.
        code: ErrorCode,
        /// What was wrong, for the log.
        why: &'static str,
    },
    /// Resets one stream; the connection carries on.
    Stream {
        /// The stream to reset.
        stream: u32,
        /// The code sent with the RST_STREAM.
        code: ErrorCode,
        /// What was wrong, for the log.
        why: &'static str,
    },
}

impl H2Error {
    /// The error code sent with the RST_STREAM or GOAWAY.
    #[must_use]
    pub fn code(&self) -> ErrorCode {
        match self {
            H2Error::Connection { code, .. } | H2Error::Stream { code, .. } => *code,
        }
    }
}

impl std::fmt::Display for H2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            H2Error::Connection { code, why } => {
                write!(f, "HTTP/2 connection error {code:?}: {why}")
            }
            H2Error::Stream { stream, code, why } => {
                write!(f, "HTTP/2 stream {stream} error {code:?}: {why}")
            }
        }
    }
}

impl std::error::Error for H2Error {}
