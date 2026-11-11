//! HTTP/1.1 (RFC 9112), sans-I/O: parsing request heads, decoding chunked
//! bodies, and encoding responses. HTTP/1.1 only (DECISIONS D217).

mod chunked;
mod encode;
mod request;

#[cfg(test)]
mod tests;

pub use chunked::{Chunk, ChunkError, ChunkedDecoder};
pub use encode::{
    CHUNK_END, CONTINUE, ChunkHeader, EncodeError, Framing, chunk_header, encode_last_chunk,
    encode_response_head, reason,
};
pub use request::{Body, Limits, Parsed, RequestError, RequestHead, TargetForm, parse_request};
