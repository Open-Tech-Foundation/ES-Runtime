//! HTTP/1.1 (RFC 9112), sans-I/O, for both roles: the server parses request
//! heads and encodes responses, the client encodes requests and parses
//! responses, and both decode chunked bodies with the same decoder. HTTP/1.1
//! only (DECISIONS D217).

mod chunked;
mod encode;
mod request;
mod response;

#[cfg(test)]
mod tests;

pub use chunked::{Chunk, ChunkError, ChunkedDecoder};
pub use encode::{
    CHUNK_END, CONTINUE, ChunkHeader, EncodeError, Framing, chunk_header, encode_last_chunk,
    encode_request_head, encode_response_head, reason,
};
pub use request::{Body, Limits, Parsed, RequestError, RequestHead, TargetForm, parse_request};
pub use response::{ParsedResponse, ResponseBody, ResponseError, ResponseHead, parse_response};
