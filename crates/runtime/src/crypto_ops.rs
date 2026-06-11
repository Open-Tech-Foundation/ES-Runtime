//! Host ops backing WebCrypto (SPEC §2.10), using the RustCrypto suite —
//! vetted, constant-time primitives (DECISIONS.md D9; §7 forbids hand-rolled
//! crypto).
//!
//! `random_bytes` draws from the [`Entropy`] provider; the `subtle_*` ops are
//! pure computation. They are synchronous (crypto is fast for typical sizes);
//! the prelude's `crypto.subtle` wraps each in a Promise. Offloading large
//! operations via `TaskSpawner` is a later refinement.
//!
//! Phase 7 ships digest, HMAC, and AES-GCM; ECDSA/ECDH/RSA are staged (SPEC §7).

use std::sync::Arc;

use es_runtime_common::{ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::Entropy;

use aes_gcm::aead::{Aead, KeyInit as AeadKeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use hmac::digest::KeyInit as MacKeyInit;
use hmac::{Hmac, Mac};
use sha2::Digest as _;

use crate::Result;

/// Registers the WebCrypto host ops.
pub(crate) fn install(engine: &mut dyn Engine, entropy: Arc<dyn Entropy>) -> Result<()> {
    engine.register_op(OpDecl::sync("random_bytes", move |args| {
        let len = args.first().and_then(Value::as_number).unwrap_or(0.0) as usize;
        let mut buf = vec![0u8; len];
        entropy
            .fill(&mut buf)
            .map_err(|e| OpError::new(e.exception_class(), e.exception_message()))?;
        Ok(Value::Bytes(buf))
    }))?;

    engine.register_op(OpDecl::sync("subtle_digest", |args| {
        let alg = arg_str(&args, 0)?;
        let data = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(digest(&alg, &data)?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_hmac_sign", |args| {
        let hash = arg_str(&args, 0)?;
        let key = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        Ok(Value::Bytes(hmac_sign(&hash, &key, &data)?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_hmac_verify", |args| {
        let hash = arg_str(&args, 0)?;
        let key = arg_bytes(&args, 1)?;
        let signature = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bool(hmac_verify(&hash, &key, &signature, &data)?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_aes_gcm_encrypt", |args| {
        let key = arg_bytes(&args, 0)?;
        let iv = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        let aad = arg_bytes(&args, 3).unwrap_or_default();
        Ok(Value::Bytes(aes_gcm_seal(&key, &iv, &data, &aad)?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_aes_gcm_decrypt", |args| {
        let key = arg_bytes(&args, 0)?;
        let iv = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        let aad = arg_bytes(&args, 3).unwrap_or_default();
        Ok(Value::Bytes(aes_gcm_open(&key, &iv, &data, &aad)?))
    }))?;

    Ok(())
}

fn arg_str(args: &[Value], i: usize) -> std::result::Result<String, OpError> {
    args.get(i)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| type_error(format!("argument {i} must be a string")))
}

fn arg_bytes(args: &[Value], i: usize) -> std::result::Result<Vec<u8>, OpError> {
    args.get(i)
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| type_error(format!("argument {i} must be a BufferSource")))
}

fn type_error(message: String) -> OpError {
    OpError::new(ExceptionClass::TypeError, message)
}

/// `NotSupportedError` for an unknown algorithm.
fn not_supported(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::DomException("NotSupportedError"), message)
}

/// `OperationError` for a crypto-operation failure (bad key length, auth tag
/// mismatch, …).
fn operation_error(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::DomException("OperationError"), message)
}

fn digest(alg: &str, data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    Ok(match alg {
        "SHA-1" => sha1::Sha1::digest(data).to_vec(),
        "SHA-256" => sha2::Sha256::digest(data).to_vec(),
        "SHA-384" => sha2::Sha384::digest(data).to_vec(),
        "SHA-512" => sha2::Sha512::digest(data).to_vec(),
        other => return Err(not_supported(format!("unsupported digest: {other}"))),
    })
}

fn hmac_sign(hash: &str, key: &[u8], data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    match hash {
        "SHA-1" => mac_sign::<Hmac<sha1::Sha1>>(key, data),
        "SHA-256" => mac_sign::<Hmac<sha2::Sha256>>(key, data),
        "SHA-384" => mac_sign::<Hmac<sha2::Sha384>>(key, data),
        "SHA-512" => mac_sign::<Hmac<sha2::Sha512>>(key, data),
        other => Err(not_supported(format!("unsupported HMAC hash: {other}"))),
    }
}

fn mac_sign<M: Mac + MacKeyInit>(key: &[u8], data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let mut mac = <M as MacKeyInit>::new_from_slice(key)
        .map_err(|_| operation_error("invalid HMAC key length"))?;
    mac.update(data);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hmac_verify(
    hash: &str,
    key: &[u8],
    signature: &[u8],
    data: &[u8],
) -> std::result::Result<bool, OpError> {
    match hash {
        "SHA-1" => mac_verify::<Hmac<sha1::Sha1>>(key, signature, data),
        "SHA-256" => mac_verify::<Hmac<sha2::Sha256>>(key, signature, data),
        "SHA-384" => mac_verify::<Hmac<sha2::Sha384>>(key, signature, data),
        "SHA-512" => mac_verify::<Hmac<sha2::Sha512>>(key, signature, data),
        other => Err(not_supported(format!("unsupported HMAC hash: {other}"))),
    }
}

fn mac_verify<M: Mac + MacKeyInit>(
    key: &[u8],
    signature: &[u8],
    data: &[u8],
) -> std::result::Result<bool, OpError> {
    let mut mac = <M as MacKeyInit>::new_from_slice(key)
        .map_err(|_| operation_error("invalid HMAC key length"))?;
    mac.update(data);
    // `verify_slice` is constant-time.
    Ok(mac.verify_slice(signature).is_ok())
}

fn aes_gcm_seal(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    aad: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    if iv.len() != 12 {
        return Err(operation_error("AES-GCM IV must be 12 bytes"));
    }
    let payload = Payload { msg: data, aad };
    let nonce = Nonce::from_slice(iv);
    match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .map_err(|_| operation_error("invalid AES-128 key"))?
            .encrypt(nonce, payload)
            .map_err(|_| operation_error("encryption failed")),
        32 => Aes256Gcm::new_from_slice(key)
            .map_err(|_| operation_error("invalid AES-256 key"))?
            .encrypt(nonce, payload)
            .map_err(|_| operation_error("encryption failed")),
        _ => Err(operation_error("AES-GCM key must be 16 or 32 bytes")),
    }
}

fn aes_gcm_open(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
    aad: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    if iv.len() != 12 {
        return Err(operation_error("AES-GCM IV must be 12 bytes"));
    }
    let payload = Payload { msg: data, aad };
    let nonce = Nonce::from_slice(iv);
    match key.len() {
        16 => Aes128Gcm::new_from_slice(key)
            .map_err(|_| operation_error("invalid AES-128 key"))?
            .decrypt(nonce, payload)
            .map_err(|_| operation_error("decryption failed")),
        32 => Aes256Gcm::new_from_slice(key)
            .map_err(|_| operation_error("invalid AES-256 key"))?
            .decrypt(nonce, payload)
            .map_err(|_| operation_error("decryption failed")),
        _ => Err(operation_error("AES-GCM key must be 16 or 32 bytes")),
    }
}
