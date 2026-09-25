//! `__utf8_decode(view, fatal, ignoreBOM)`: `TextDecoder`'s one-shot UTF-8
//! path, as a builtin rather than an op.
//!
//! A decode is the smallest thing the runtime is asked to do and among the most
//! frequent — every text column of every database row is one. Through the op
//! boundary it cost a fixed ~300ns whatever the input: the arguments are
//! marshaled into `Value`s, which *copies* the bytes into a `Vec`, and the label
//! is resolved again on every call. Here the bytes are read where they lie and
//! V8 builds the string straight from them, which is the whole of the work.
//!
//! Only UTF-8, and only one-shot. Every other encoding, and every streaming
//! decode, keeps the `encoding_rs` op path in `runtime`: they carry state or a
//! transcoding step, and neither is on anybody's hot path.

use es_runtime_common::ExceptionClass;

use crate::convert::throw;
use crate::error::Result;
use crate::op::OpError;

const BOM: &[u8] = &[0xef, 0xbb, 0xbf];

pub(crate) fn install(scope: &mut v8::PinScope, context: v8::Local<v8::Context>) -> Result<()> {
    let global = context.global(scope);
    crate::op::install_global_fn(scope, global, "__utf8_decode", utf8_decode, None)
}

pub(crate) fn external_references() -> Vec<v8::ExternalReference> {
    use v8::MapFnTo;
    vec![v8::ExternalReference {
        function: utf8_decode.map_fn_to(),
    }]
}

/// Contains panics as a JS exception rather than unwinding across V8 (D15).
fn utf8_decode(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    rv: v8::ReturnValue<v8::Value>,
) {
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        utf8_decode_inner(&mut *scope, args, rv);
    }));
    if caught.is_err() && !scope.is_execution_terminating() {
        throw(
            scope,
            &OpError::new(ExceptionClass::Error, "internal error in __utf8_decode"),
        );
    }
}

fn utf8_decode_inner(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue<v8::Value>,
) {
    let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(args.get(0)) else {
        throw(
            scope,
            &OpError::type_error("TextDecoder input must be a BufferSource"),
        );
        return;
    };
    let fatal = args.get(1).is_true();
    let ignore_bom = args.get(2).is_true();

    // Off-heap contents are read in place; a small on-heap typed array is copied
    // into `storage`, which is what V8 allows and is at most 64 bytes.
    let mut storage = [0u8; v8::TYPED_ARRAY_MAX_SIZE_IN_HEAP];
    let mut bytes = view.get_contents(&mut storage);
    if !ignore_bom && bytes.starts_with(BOM) {
        bytes = &bytes[BOM.len()..];
    }

    let string = if bytes.is_ascii() {
        // ASCII is Latin-1, so V8 can take the bytes as a one-byte string with
        // no decoding at all — the common case for identifiers, keys and most
        // text a program handles.
        v8::String::new_from_one_byte(scope, bytes, v8::NewStringType::Normal)
    } else {
        match std::str::from_utf8(bytes) {
            Ok(text) => {
                v8::String::new_from_utf8(scope, text.as_bytes(), v8::NewStringType::Normal)
            }
            Err(_) if fatal => {
                throw(
                    scope,
                    &OpError::type_error("the encoded data was not valid for the encoding"),
                );
                return;
            }
            // U+FFFD per maximal subpart — the WHATWG replacement rule, and the
            // one the op path already follows.
            Err(_) => {
                let text = String::from_utf8_lossy(bytes);
                v8::String::new_from_utf8(scope, text.as_bytes(), v8::NewStringType::Normal)
            }
        }
    };
    match string {
        Some(string) => rv.set(string.into()),
        None => throw(
            scope,
            &OpError::range_error("decoded text would be too large"),
        ),
    }
}
