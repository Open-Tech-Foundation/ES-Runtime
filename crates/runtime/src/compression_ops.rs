//! Host ops backing `CompressionStream` / `DecompressionStream` (Compression
//! Streams, part of the WinterTC Minimum Common API).
//!
//! Pure computation (no capability), but stateful: each JS stream owns a native
//! flate2 context that lives across chunks in a registry here, keyed by id.
//! `compression_new` allocates one for a format (`gzip` / `deflate` /
//! `deflate-raw`) and direction, `compression_write` feeds it a chunk and
//! returns whatever output the codec produced so far, `compression_finish`
//! flushes the tail and frees it, and `compression_free` discards it early (the
//! transformer's cancel hook — abort/cancel paths where flush never runs).
//! A codec error (corrupt input, trailing junk, or — at finish — truncated
//! input) surfaces as a `TypeError` and frees the context, matching the spec's
//! error-the-stream semantics. The zlib / raw-deflate decoders use the
//! low-level [`Decompress`] state machine because the write adapters accept a
//! truncated stream at finish; gzip keeps [`GzDecoder`], whose checksum check
//! already rejects truncation.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::rc::Rc;

use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use flate2::write::{DeflateEncoder, GzDecoder, GzEncoder, ZlibEncoder};
use flate2::{Compression, Decompress, FlushDecompress, Status};

/// Streaming zlib / raw-deflate decoder over the low-level state machine,
/// tracking stream end so truncation and trailing junk are detected.
struct Inflate {
    inner: Decompress,
    done: bool,
}

impl Inflate {
    fn new(zlib_header: bool) -> Inflate {
        Inflate {
            inner: Decompress::new(zlib_header),
            done: false,
        }
    }

    fn invalid(e: impl ToString) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    }

    fn write(&mut self, mut chunk: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        while !chunk.is_empty() {
            if self.done {
                return Err(Self::invalid(
                    "junk found after the end of the compressed data",
                ));
            }
            let before_in = self.inner.total_in();
            let before_out = self.inner.total_out();
            let status = self
                .inner
                .decompress(chunk, &mut buf, FlushDecompress::None)
                .map_err(Self::invalid)?;
            let consumed = (self.inner.total_in() - before_in) as usize;
            let produced = (self.inner.total_out() - before_out) as usize;
            out.extend_from_slice(&buf[..produced]);
            chunk = &chunk[consumed..];
            match status {
                Status::StreamEnd => self.done = true,
                // No progress with a fresh output buffer: nothing more to do.
                _ if consumed == 0 && produced == 0 => break,
                _ => {}
            }
        }
        Ok(out)
    }

    fn finish(mut self) -> std::io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 16 * 1024];
        while !self.done {
            let before_out = self.inner.total_out();
            let status = self
                .inner
                .decompress(&[], &mut buf, FlushDecompress::Finish)
                .map_err(Self::invalid)?;
            let produced = (self.inner.total_out() - before_out) as usize;
            out.extend_from_slice(&buf[..produced]);
            match status {
                Status::StreamEnd => self.done = true,
                _ if produced == 0 => {
                    return Err(Self::invalid("the compressed data was truncated"));
                }
                _ => {}
            }
        }
        Ok(out)
    }
}

/// One live codec: encoders are flate2 write-adapters over the output
/// accumulator; zlib / raw decode is the truncation-aware [`Inflate`].
enum Codec {
    GzipEncode(GzEncoder<Vec<u8>>),
    GzipDecode(GzDecoder<Vec<u8>>),
    ZlibEncode(ZlibEncoder<Vec<u8>>),
    RawEncode(DeflateEncoder<Vec<u8>>),
    Inflate(Inflate),
}

impl Codec {
    fn new(format: &str, decompress: bool) -> Option<Codec> {
        let level = Compression::default();
        Some(match (format, decompress) {
            ("gzip", false) => Codec::GzipEncode(GzEncoder::new(Vec::new(), level)),
            ("gzip", true) => Codec::GzipDecode(GzDecoder::new(Vec::new())),
            ("deflate", false) => Codec::ZlibEncode(ZlibEncoder::new(Vec::new(), level)),
            ("deflate", true) => Codec::Inflate(Inflate::new(true)),
            ("deflate-raw", false) => Codec::RawEncode(DeflateEncoder::new(Vec::new(), level)),
            ("deflate-raw", true) => Codec::Inflate(Inflate::new(false)),
            _ => return None,
        })
    }

    /// Feeds a chunk and drains the output produced so far.
    fn write(&mut self, chunk: &[u8]) -> std::io::Result<Vec<u8>> {
        match self {
            Codec::GzipEncode(e) => {
                e.write_all(chunk)?;
                Ok(std::mem::take(e.get_mut()))
            }
            Codec::GzipDecode(d) => {
                d.write_all(chunk)?;
                Ok(std::mem::take(d.get_mut()))
            }
            Codec::ZlibEncode(e) => {
                e.write_all(chunk)?;
                Ok(std::mem::take(e.get_mut()))
            }
            Codec::RawEncode(e) => {
                e.write_all(chunk)?;
                Ok(std::mem::take(e.get_mut()))
            }
            Codec::Inflate(d) => d.write(chunk),
        }
    }

    /// Ends the stream and returns the remaining output. For decompression this
    /// errors on a truncated stream (the spec's flush-time integrity check).
    fn finish(self) -> std::io::Result<Vec<u8>> {
        match self {
            Codec::GzipEncode(e) => e.finish(),
            Codec::GzipDecode(d) => d.finish(),
            Codec::ZlibEncode(e) => e.finish(),
            Codec::RawEncode(e) => e.finish(),
            Codec::Inflate(d) => d.finish(),
        }
    }
}

fn type_error(e: std::io::Error) -> OpError {
    OpError::new(ExceptionClass::TypeError, e.to_string())
}

/// Registers `compression_new` / `compression_write` / `compression_finish` /
/// `compression_free`.
pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    let registry: Rc<RefCell<HashMap<u64, Codec>>> = Rc::new(RefCell::new(HashMap::new()));
    let next_id = Rc::new(RefCell::new(0u64));

    let reg = registry.clone();
    engine.register_op(OpDecl::sync("compression_new", move |args| {
        let format = args.first().and_then(Value::as_str).unwrap_or("");
        let decompress = matches!(args.get(1), Some(Value::Bool(true)));
        let codec = Codec::new(format, decompress).ok_or_else(|| {
            OpError::new(
                ExceptionClass::TypeError,
                format!("Unsupported compression format: '{format}'"),
            )
        })?;
        let mut id = next_id.borrow_mut();
        *id += 1;
        reg.borrow_mut().insert(*id, codec);
        Ok(Value::Number(*id as f64))
    }))?;

    let reg = registry.clone();
    engine.register_op(OpDecl::sync("compression_write", move |args| {
        let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
        let chunk = args.get(1).and_then(Value::as_bytes).unwrap_or(&[]);
        let mut map = reg.borrow_mut();
        let codec = map.get_mut(&id).ok_or_else(|| {
            OpError::new(ExceptionClass::TypeError, "the stream is already closed")
        })?;
        match codec.write(chunk) {
            Ok(out) => Ok(Value::Bytes(out)),
            Err(e) => {
                map.remove(&id); // errored streams never flush; free eagerly
                Err(type_error(e))
            }
        }
    }))?;

    let reg = registry.clone();
    engine.register_op(OpDecl::sync("compression_finish", move |args| {
        let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
        let codec = reg.borrow_mut().remove(&id).ok_or_else(|| {
            OpError::new(ExceptionClass::TypeError, "the stream is already closed")
        })?;
        codec.finish().map(Value::Bytes).map_err(type_error)
    }))?;

    engine.register_op(OpDecl::sync("compression_free", move |args| {
        let id = args.first().and_then(Value::as_number).unwrap_or(0.0) as u64;
        registry.borrow_mut().remove(&id);
        Ok(Value::Undefined)
    }))?;

    Ok(())
}
