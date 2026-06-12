//! Host ops backing WebCrypto (SPEC §2.10), using the RustCrypto suite —
//! vetted, constant-time primitives (DECISIONS.md D9; §7 forbids hand-rolled
//! crypto).
//!
//! `random_bytes` draws from the [`Entropy`] provider; the `subtle_*` ops are
//! pure computation. They are synchronous (crypto is fast for typical sizes);
//! the prelude's `crypto.subtle` wraps each in a Promise. Offloading large
//! operations via `TaskSpawner` is a later refinement.
//!
//! Phase 7 ships digest, HMAC, and AES-GCM; Phase 7b adds AES-CBC, AES-CTR,
//! and the HKDF/PBKDF2 key-derivation functions. Elliptic-curve ECDSA/ECDH
//! live in the sibling [`crate::ec_ops`] module; RSA is staged (SPEC §7).

use std::sync::Arc;

use es_runtime_common::{ExceptionClass, IntoException};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::Entropy;

use aes::{Aes128, Aes192, Aes256};
use aes_gcm::aead::{Aead, KeyInit as AeadKeyInit, Payload};
use aes_gcm::{Aes128Gcm, Aes256Gcm, Nonce};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use hmac::digest::KeyInit as MacKeyInit;
use hmac::{Hmac, Mac};
use pbkdf2::pbkdf2_hmac;
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

    engine.register_op(OpDecl::sync("subtle_aes_cbc_encrypt", |args| {
        let key = arg_bytes(&args, 0)?;
        let iv = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        Ok(Value::Bytes(aes_cbc_encrypt(&key, &iv, &data)?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_aes_cbc_decrypt", |args| {
        let key = arg_bytes(&args, 0)?;
        let iv = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        Ok(Value::Bytes(aes_cbc_decrypt(&key, &iv, &data)?))
    }))?;

    // AES-CTR is symmetric: the same keystream XOR serves encrypt and decrypt,
    // so one op backs both `subtle.encrypt` and `subtle.decrypt`.
    engine.register_op(OpDecl::sync("subtle_aes_ctr", |args| {
        let key = arg_bytes(&args, 0)?;
        let counter = arg_bytes(&args, 1)?;
        let length = args.get(2).and_then(Value::as_number).unwrap_or(0.0) as usize;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bytes(aes_ctr(&key, &counter, length, &data)?))
    }))?;

    // Key derivation (`deriveBits`/`deriveKey`). `length` is in bytes — the
    // prelude converts from the spec's bit length.
    engine.register_op(OpDecl::sync("subtle_hkdf", |args| {
        let hash = arg_str(&args, 0)?;
        let ikm = arg_bytes(&args, 1)?;
        let salt = arg_bytes(&args, 2)?;
        let info = arg_bytes(&args, 3)?;
        let length = args.get(4).and_then(Value::as_number).unwrap_or(0.0) as usize;
        Ok(Value::Bytes(hkdf_derive(
            &hash, &ikm, &salt, &info, length,
        )?))
    }))?;

    engine.register_op(OpDecl::sync("subtle_pbkdf2", |args| {
        let hash = arg_str(&args, 0)?;
        let password = arg_bytes(&args, 1)?;
        let salt = arg_bytes(&args, 2)?;
        let iterations = args.get(3).and_then(Value::as_number).unwrap_or(0.0) as u32;
        let length = args.get(4).and_then(Value::as_number).unwrap_or(0.0) as usize;
        Ok(Value::Bytes(pbkdf2_derive(
            &hash, &password, &salt, iterations, length,
        )?))
    }))?;

    Ok(())
}

pub(crate) fn arg_str(args: &[Value], i: usize) -> std::result::Result<String, OpError> {
    args.get(i)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| type_error(format!("argument {i} must be a string")))
}

pub(crate) fn arg_bytes(args: &[Value], i: usize) -> std::result::Result<Vec<u8>, OpError> {
    args.get(i)
        .and_then(Value::as_bytes)
        .map(<[u8]>::to_vec)
        .ok_or_else(|| type_error(format!("argument {i} must be a BufferSource")))
}

fn type_error(message: String) -> OpError {
    OpError::new(ExceptionClass::TypeError, message)
}

/// `NotSupportedError` for an unknown algorithm.
pub(crate) fn not_supported(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::DomException("NotSupportedError"), message)
}

/// `OperationError` for a crypto-operation failure (bad key length, auth tag
/// mismatch, …).
pub(crate) fn operation_error(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::DomException("OperationError"), message)
}

/// `DataError` for malformed key material (bad DER, wrong point encoding, …).
pub(crate) fn data_error(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::DomException("DataError"), message)
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

// ---- AES-CBC (PKCS#7 padding, per WebCrypto) -------------------------------

fn cbc_encrypt<C: KeyIvInit + BlockEncryptMut>(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let enc = C::new_from_slices(key, iv).map_err(|_| operation_error("invalid AES-CBC key/iv"))?;
    Ok(enc.encrypt_padded_vec_mut::<Pkcs7>(data))
}

fn cbc_decrypt<C: KeyIvInit + BlockDecryptMut>(
    key: &[u8],
    iv: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let dec = C::new_from_slices(key, iv).map_err(|_| operation_error("invalid AES-CBC key/iv"))?;
    dec.decrypt_padded_vec_mut::<Pkcs7>(data)
        .map_err(|_| operation_error("invalid AES-CBC padding"))
}

fn aes_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    if iv.len() != 16 {
        return Err(operation_error("AES-CBC IV must be 16 bytes"));
    }
    match key.len() {
        16 => cbc_encrypt::<cbc::Encryptor<Aes128>>(key, iv, data),
        24 => cbc_encrypt::<cbc::Encryptor<Aes192>>(key, iv, data),
        32 => cbc_encrypt::<cbc::Encryptor<Aes256>>(key, iv, data),
        _ => Err(operation_error("AES-CBC key must be 16, 24, or 32 bytes")),
    }
}

fn aes_cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    if iv.len() != 16 {
        return Err(operation_error("AES-CBC IV must be 16 bytes"));
    }
    match key.len() {
        16 => cbc_decrypt::<cbc::Decryptor<Aes128>>(key, iv, data),
        24 => cbc_decrypt::<cbc::Decryptor<Aes192>>(key, iv, data),
        32 => cbc_decrypt::<cbc::Decryptor<Aes256>>(key, iv, data),
        _ => Err(operation_error("AES-CBC key must be 16, 24, or 32 bytes")),
    }
}

// ---- AES-CTR ---------------------------------------------------------------
//
// WebCrypto's `length` selects how many low-order bits of the 16-byte counter
// block increment; the rest is a fixed nonce. RustCrypto exposes fixed-width
// big-endian counters, so we support the common 32/64/128-bit widths and reject
// others as `NotSupportedError`.

fn ctr_apply<C: KeyIvInit + StreamCipher>(
    key: &[u8],
    counter: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let mut cipher = C::new_from_slices(key, counter)
        .map_err(|_| operation_error("invalid AES-CTR key/counter"))?;
    let mut buf = data.to_vec();
    cipher.apply_keystream(&mut buf);
    Ok(buf)
}

fn aes_ctr(
    key: &[u8],
    counter: &[u8],
    length: usize,
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    if counter.len() != 16 {
        return Err(operation_error("AES-CTR counter must be 16 bytes"));
    }
    match (key.len(), length) {
        (16, 128) => ctr_apply::<ctr::Ctr128BE<Aes128>>(key, counter, data),
        (24, 128) => ctr_apply::<ctr::Ctr128BE<Aes192>>(key, counter, data),
        (32, 128) => ctr_apply::<ctr::Ctr128BE<Aes256>>(key, counter, data),
        (16, 64) => ctr_apply::<ctr::Ctr64BE<Aes128>>(key, counter, data),
        (24, 64) => ctr_apply::<ctr::Ctr64BE<Aes192>>(key, counter, data),
        (32, 64) => ctr_apply::<ctr::Ctr64BE<Aes256>>(key, counter, data),
        (16, 32) => ctr_apply::<ctr::Ctr32BE<Aes128>>(key, counter, data),
        (24, 32) => ctr_apply::<ctr::Ctr32BE<Aes192>>(key, counter, data),
        (32, 32) => ctr_apply::<ctr::Ctr32BE<Aes256>>(key, counter, data),
        (_, 32 | 64 | 128) => Err(operation_error("AES-CTR key must be 16, 24, or 32 bytes")),
        _ => Err(not_supported(
            "AES-CTR counter length must be 32, 64, or 128 bits",
        )),
    }
}

// ---- Key derivation: HKDF (RFC 5869) and PBKDF2 (RFC 8018) -----------------

fn hkdf_derive(
    hash: &str,
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    length: usize,
) -> std::result::Result<Vec<u8>, OpError> {
    let mut okm = vec![0u8; length];
    let ok = match hash {
        "SHA-1" => Hkdf::<sha1::Sha1>::new(Some(salt), ikm)
            .expand(info, &mut okm)
            .is_ok(),
        "SHA-256" => Hkdf::<sha2::Sha256>::new(Some(salt), ikm)
            .expand(info, &mut okm)
            .is_ok(),
        "SHA-384" => Hkdf::<sha2::Sha384>::new(Some(salt), ikm)
            .expand(info, &mut okm)
            .is_ok(),
        "SHA-512" => Hkdf::<sha2::Sha512>::new(Some(salt), ikm)
            .expand(info, &mut okm)
            .is_ok(),
        other => return Err(not_supported(format!("unsupported HKDF hash: {other}"))),
    };
    // `expand` only fails when the requested length exceeds 255 * HashLen.
    if !ok {
        return Err(operation_error("HKDF output length too large"));
    }
    Ok(okm)
}

fn pbkdf2_derive(
    hash: &str,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    length: usize,
) -> std::result::Result<Vec<u8>, OpError> {
    if iterations == 0 {
        return Err(operation_error("PBKDF2 iterations must be at least 1"));
    }
    let mut out = vec![0u8; length];
    match hash {
        "SHA-1" => pbkdf2_hmac::<sha1::Sha1>(password, salt, iterations, &mut out),
        "SHA-256" => pbkdf2_hmac::<sha2::Sha256>(password, salt, iterations, &mut out),
        "SHA-384" => pbkdf2_hmac::<sha2::Sha384>(password, salt, iterations, &mut out),
        "SHA-512" => pbkdf2_hmac::<sha2::Sha512>(password, salt, iterations, &mut out),
        other => return Err(not_supported(format!("unsupported PBKDF2 hash: {other}"))),
    }
    Ok(out)
}
