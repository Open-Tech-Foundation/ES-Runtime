//! Host ops backing the `runtime:hashing` module (DECISIONS D57).
//!
//! Pure computation, and therefore ungated — like [`compression_ops`] and
//! [`serialization_ops`], hashing reads nothing and reaches nothing. What a
//! caller learns from `hash("sha256", x)` is a fact about `x`, which they
//! already hold.
//!
//! [`compression_ops`]: crate::compression_ops
//! [`serialization_ops`]: crate::serialization_ops
//!
//! Three families live here, all absent from `crypto.subtle` (D9):
//!
//!   * **Digests** it does not carry — SHA-3, BLAKE3, MD5, RIPEMD-160 — plus
//!     the SHA-2 family it does, so one API covers every algorithm rather than
//!     two that split by vintage. Offered both one-shot and incremental: a
//!     `Hasher` id names a native state that lives across `update` calls, so a
//!     multi-gigabyte file is hashed as it streams instead of being buffered to
//!     satisfy `subtle.digest`'s one-shot signature.
//!   * **Non-cryptographic hashes** — xxHash, CRC-32, CRC-32C — for the cache
//!     keys, ETags and shard selections a server computes constantly and which
//!     a cryptographic hash answers at ten times the cost.
//!   * **Password hashing** — Argon2id, bcrypt, scrypt — where `subtle` offers
//!     only PBKDF2, and which is the one thing on this list a server *must not*
//!     get wrong.
//!
//! **Encoding is done here, not in JS.** Every digest op takes an encoding and
//! can return `hex`/`base64`/`base64url` directly, because the alternative is
//! the loop every codebase writes once and copies forever
//! (`Array.from(bytes).map(b => b.toString(16).padStart(2, "0")).join("")`),
//! which allocates a string per byte to produce a string the host already had.
//!
//! **Salts come from JS, not from here.** A password hash needs a random salt,
//! and randomness in this runtime is the `Entropy` provider's to give (D9), not
//! something an op helps itself to. So the module draws the salt with
//! `crypto.getRandomValues` and passes it in: hashing a password inherits the
//! Entropy gate exactly as it should, verifying one needs no randomness at all
//! (the salt is in the stored string) and therefore no capability, and these
//! ops stay pure functions of their arguments.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use base64::{
    Engine as _, engine::general_purpose::STANDARD, engine::general_purpose::URL_SAFE_NO_PAD,
};
use es_runtime_common::ExceptionClass;
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use hmac::{Hmac, Mac};
use sha2::Digest as _;
use std::hash::Hasher as _;
use subtle::ConstantTimeEq as _;

/// Every algorithm `hash()` and `new Hasher()` accept — the cryptographic ones
/// first, then the checksums. The split matters at one place only, `hmac`,
/// which refuses the checksums outright: a MAC built on CRC-32 is not a MAC.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Alg {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Sha3_224,
    Sha3_256,
    Sha3_384,
    Sha3_512,
    Blake3,
    Md5,
    Ripemd160,
    XxHash64,
    XxHash3,
    Crc32,
    Crc32c,
}

impl Alg {
    /// The canonical spelling of every algorithm, in the order the docs list
    /// them. Also the error message's suggestion list, so an unknown name is
    /// answered with the known ones rather than a bare refusal.
    const ALL: [(&'static str, Alg); 15] = [
        ("sha1", Alg::Sha1),
        ("sha256", Alg::Sha256),
        ("sha384", Alg::Sha384),
        ("sha512", Alg::Sha512),
        ("sha3-224", Alg::Sha3_224),
        ("sha3-256", Alg::Sha3_256),
        ("sha3-384", Alg::Sha3_384),
        ("sha3-512", Alg::Sha3_512),
        ("blake3", Alg::Blake3),
        ("md5", Alg::Md5),
        ("ripemd160", Alg::Ripemd160),
        ("xxhash64", Alg::XxHash64),
        ("xxhash3", Alg::XxHash3),
        ("crc32", Alg::Crc32),
        ("crc32c", Alg::Crc32c),
    ];

    /// Parses an algorithm name. Case-insensitive, and `SHA-256` is accepted
    /// beside `sha256` so a WebCrypto name (`subtle.digest("SHA-256", …)`)
    /// works unchanged — the two APIs should not disagree about what a hash is
    /// called. `-` is significant only in `sha3-*`, where it separates the
    /// family from the length, so it is stripped everywhere else.
    fn parse(name: &str) -> Option<Alg> {
        let lower = name.to_ascii_lowercase();
        let canonical = if lower.starts_with("sha3") {
            lower
        } else {
            lower.replace('-', "")
        };
        Alg::ALL
            .iter()
            .find(|(n, _)| *n == canonical)
            .map(|(_, a)| *a)
    }

    /// The canonical name, for error messages — so a refusal names the
    /// algorithm the way the caller would write it, not the way Rust spells it.
    fn name(self) -> &'static str {
        Alg::ALL
            .iter()
            .find(|(_, a)| *a == self)
            .map(|(n, _)| *n)
            .unwrap_or("unknown")
    }

    /// The digest, computed in one pass.
    fn oneshot(self, data: &[u8]) -> Vec<u8> {
        match self {
            Alg::Sha1 => sha1::Sha1::digest(data).to_vec(),
            Alg::Sha256 => sha2::Sha256::digest(data).to_vec(),
            Alg::Sha384 => sha2::Sha384::digest(data).to_vec(),
            Alg::Sha512 => sha2::Sha512::digest(data).to_vec(),
            Alg::Sha3_224 => sha3::Sha3_224::digest(data).to_vec(),
            Alg::Sha3_256 => sha3::Sha3_256::digest(data).to_vec(),
            Alg::Sha3_384 => sha3::Sha3_384::digest(data).to_vec(),
            Alg::Sha3_512 => sha3::Sha3_512::digest(data).to_vec(),
            Alg::Blake3 => blake3::hash(data).as_bytes().to_vec(),
            Alg::Md5 => md5::Md5::digest(data).to_vec(),
            Alg::Ripemd160 => ripemd::Ripemd160::digest(data).to_vec(),
            // Big-endian, so the hex form reads as the number the algorithm
            // names — `xxhash64` of "" is `ef46db3751d8e999`, as its own test
            // vectors write it, not the byte-swapped mirror.
            Alg::XxHash64 => twox_hash::XxHash64::oneshot(0, data).to_be_bytes().to_vec(),
            Alg::XxHash3 => twox_hash::XxHash3_64::oneshot(data).to_be_bytes().to_vec(),
            Alg::Crc32 => crc32fast::hash(data).to_be_bytes().to_vec(),
            Alg::Crc32c => crc32c::crc32c(data).to_be_bytes().to_vec(),
        }
    }
}

/// A hash in progress: the native state a JS `Hasher` id names.
///
/// One variant per algorithm rather than a boxed `dyn Digest`, because the
/// non-cryptographic hashes are not `Digest` implementors at all and the
/// cryptographic ones span two traits' worth of associated types.
enum State {
    Sha1(sha1::Sha1),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
    Sha3_224(sha3::Sha3_224),
    Sha3_256(sha3::Sha3_256),
    Sha3_384(sha3::Sha3_384),
    Sha3_512(sha3::Sha3_512),
    Blake3(Box<blake3::Hasher>),
    Md5(md5::Md5),
    Ripemd160(ripemd::Ripemd160),
    XxHash64(twox_hash::XxHash64),
    XxHash3(Box<twox_hash::XxHash3_64>),
    Crc32(crc32fast::Hasher),
    Crc32c(u32),
}

impl State {
    fn new(alg: Alg) -> State {
        match alg {
            Alg::Sha1 => State::Sha1(sha1::Sha1::new()),
            Alg::Sha256 => State::Sha256(sha2::Sha256::new()),
            Alg::Sha384 => State::Sha384(sha2::Sha384::new()),
            Alg::Sha512 => State::Sha512(sha2::Sha512::new()),
            Alg::Sha3_224 => State::Sha3_224(sha3::Sha3_224::new()),
            Alg::Sha3_256 => State::Sha3_256(sha3::Sha3_256::new()),
            Alg::Sha3_384 => State::Sha3_384(sha3::Sha3_384::new()),
            Alg::Sha3_512 => State::Sha3_512(sha3::Sha3_512::new()),
            Alg::Blake3 => State::Blake3(Box::default()),
            Alg::Md5 => State::Md5(md5::Md5::new()),
            Alg::Ripemd160 => State::Ripemd160(ripemd::Ripemd160::new()),
            Alg::XxHash64 => State::XxHash64(twox_hash::XxHash64::with_seed(0)),
            Alg::XxHash3 => State::XxHash3(Box::new(twox_hash::XxHash3_64::new())),
            Alg::Crc32 => State::Crc32(crc32fast::Hasher::new()),
            Alg::Crc32c => State::Crc32c(0),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            State::Sha1(h) => h.update(data),
            State::Sha256(h) => h.update(data),
            State::Sha384(h) => h.update(data),
            State::Sha512(h) => h.update(data),
            State::Sha3_224(h) => h.update(data),
            State::Sha3_256(h) => h.update(data),
            State::Sha3_384(h) => h.update(data),
            State::Sha3_512(h) => h.update(data),
            State::Blake3(h) => {
                h.update(data);
            }
            State::Md5(h) => h.update(data),
            State::Ripemd160(h) => h.update(data),
            State::XxHash64(h) => h.write(data),
            State::XxHash3(h) => h.write(data),
            State::Crc32(h) => h.update(data),
            State::Crc32c(crc) => *crc = crc32c::crc32c_append(*crc, data),
        }
    }

    fn finish(self) -> Vec<u8> {
        match self {
            State::Sha1(h) => h.finalize().to_vec(),
            State::Sha256(h) => h.finalize().to_vec(),
            State::Sha384(h) => h.finalize().to_vec(),
            State::Sha512(h) => h.finalize().to_vec(),
            State::Sha3_224(h) => h.finalize().to_vec(),
            State::Sha3_256(h) => h.finalize().to_vec(),
            State::Sha3_384(h) => h.finalize().to_vec(),
            State::Sha3_512(h) => h.finalize().to_vec(),
            State::Blake3(h) => h.finalize().as_bytes().to_vec(),
            State::Md5(h) => h.finalize().to_vec(),
            State::Ripemd160(h) => h.finalize().to_vec(),
            State::XxHash64(h) => h.finish().to_be_bytes().to_vec(),
            State::XxHash3(h) => h.finish().to_be_bytes().to_vec(),
            State::Crc32(h) => h.finalize().to_be_bytes().to_vec(),
            State::Crc32c(crc) => crc.to_be_bytes().to_vec(),
        }
    }
}

/// How a digest leaves the host.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Bytes,
    Hex,
    Base64,
    Base64Url,
}

impl Encoding {
    fn parse(name: Option<&str>) -> Option<Encoding> {
        match name {
            None | Some("") | Some("bytes") => Some(Encoding::Bytes),
            Some("hex") => Some(Encoding::Hex),
            Some("base64") => Some(Encoding::Base64),
            Some("base64url") => Some(Encoding::Base64Url),
            _ => None,
        }
    }

    /// Encodes here rather than in JS: a hex digest is one allocation of a
    /// known size, where the JS idiom is one string per byte plus a join.
    fn apply(self, bytes: Vec<u8>) -> Value {
        match self {
            Encoding::Bytes => Value::Bytes(bytes),
            Encoding::Hex => {
                let mut out = String::with_capacity(bytes.len() * 2);
                for b in &bytes {
                    out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0'));
                    out.push(char::from_digit((b & 0x0f) as u32, 16).unwrap_or('0'));
                }
                Value::String(out)
            }
            Encoding::Base64 => Value::String(STANDARD.encode(&bytes)),
            Encoding::Base64Url => Value::String(URL_SAFE_NO_PAD.encode(&bytes)),
        }
    }
}

fn type_error(message: impl Into<String>) -> OpError {
    OpError::new(ExceptionClass::TypeError, message)
}

/// The algorithm argument at `index`, or a refusal naming what is available.
fn alg_arg(args: &[Value], index: usize) -> Result<Alg, OpError> {
    let name = args.get(index).and_then(Value::as_str).unwrap_or("");
    Alg::parse(name).ok_or_else(|| {
        let known = Alg::ALL
            .iter()
            .map(|(n, _)| *n)
            .collect::<Vec<_>>()
            .join(", ");
        type_error(format!(
            "unknown hash algorithm '{name}' (expected one of: {known})"
        ))
    })
}

/// The encoding argument at `index`, or a refusal.
fn encoding_arg(args: &[Value], index: usize) -> Result<Encoding, OpError> {
    let name = args.get(index).and_then(Value::as_str);
    Encoding::parse(name).ok_or_else(|| {
        type_error(format!(
            "unknown encoding '{}' (expected 'hex', 'base64', 'base64url', or none for bytes)",
            name.unwrap_or("")
        ))
    })
}

/// The bytes argument at `index`. A string is its UTF-8 bytes, which is what
/// `TextEncoder` would have produced a step earlier.
/// Taken out of the argument list rather than cloned: a digest's input is the
/// largest thing crossing this boundary, and marshaling already copied it once.
fn bytes_arg(args: &mut [Value], index: usize) -> Result<Vec<u8>, OpError> {
    args.get_mut(index)
        .map(|slot| std::mem::replace(slot, Value::Undefined))
        .and_then(Value::into_bytes)
        .ok_or_else(|| type_error("expected a string or a BufferSource"))
}

fn id_arg(args: &[Value], index: usize) -> u64 {
    args.get(index).and_then(Value::as_number).unwrap_or(0.0) as u64
}

// ---- HMAC ------------------------------------------------------------------

/// HMAC over any cryptographic hash here, including the three
/// `crypto.subtle` has no name for (SHA-3, RIPEMD-160, MD5 for CRAM-MD5).
fn hmac_sign(alg: Alg, key: &[u8], data: &[u8]) -> Result<Vec<u8>, OpError> {
    fn sign<M: Mac + hmac::digest::KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
        let mut mac = <M as hmac::digest::KeyInit>::new_from_slice(key)
            .expect("HMAC accepts a key of any length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
    }

    Ok(match alg {
        Alg::Sha1 => sign::<Hmac<sha1::Sha1>>(key, data),
        Alg::Sha256 => sign::<Hmac<sha2::Sha256>>(key, data),
        Alg::Sha384 => sign::<Hmac<sha2::Sha384>>(key, data),
        Alg::Sha512 => sign::<Hmac<sha2::Sha512>>(key, data),
        Alg::Sha3_224 => sign::<Hmac<sha3::Sha3_224>>(key, data),
        Alg::Sha3_256 => sign::<Hmac<sha3::Sha3_256>>(key, data),
        Alg::Sha3_384 => sign::<Hmac<sha3::Sha3_384>>(key, data),
        Alg::Sha3_512 => sign::<Hmac<sha3::Sha3_512>>(key, data),
        Alg::Md5 => sign::<Hmac<md5::Md5>>(key, data),
        Alg::Ripemd160 => sign::<Hmac<ripemd::Ripemd160>>(key, data),
        // BLAKE3 has its own keyed mode (a 32-byte key, not HMAC's padding
        // construction), so nesting it inside HMAC would be the wrong
        // primitive under the right name.
        Alg::Blake3 => {
            return Err(type_error(
                "blake3 has its own keyed mode and is not available through hmac",
            ));
        }
        Alg::XxHash64 | Alg::XxHash3 | Alg::Crc32 | Alg::Crc32c => {
            return Err(type_error(format!(
                "hmac needs a cryptographic hash, and '{}' is a checksum",
                alg.name()
            )));
        }
    })
}

// ---- password hashing ------------------------------------------------------

/// The parameters a password hash is computed with, read from the options
/// object the JS side normalizes. Defaults are the OWASP-recommended settings
/// as of 2026, and live in JS so they are documented where they are chosen.
struct PwParams {
    /// Argon2: KiB of memory. scrypt: unused.
    memory: u32,
    /// Argon2: passes. bcrypt/scrypt: unused.
    time: u32,
    /// Argon2/scrypt: lanes.
    parallelism: u32,
    /// bcrypt: log2 rounds. scrypt: log2 of N.
    cost: u32,
    /// scrypt: block size r.
    block_size: u32,
}

fn u32_field(fields: &[(String, Value)], name: &str, fallback: u32) -> u32 {
    fields
        .iter()
        .find(|(k, _)| k == name)
        .and_then(|(_, v)| v.as_number())
        .map(|n| n as u32)
        .unwrap_or(fallback)
}

fn password_hash(
    algorithm: &str,
    password: &[u8],
    salt: &[u8],
    params: PwParams,
) -> Result<String, OpError> {
    match algorithm {
        "argon2id" | "argon2i" | "argon2d" => {
            use argon2::password_hash::{PasswordHasher, SaltString};
            use argon2::{Algorithm, Argon2, Params, Version};

            let variant = match algorithm {
                "argon2i" => Algorithm::Argon2i,
                "argon2d" => Algorithm::Argon2d,
                _ => Algorithm::Argon2id,
            };
            let params = Params::new(params.memory, params.time, params.parallelism, None)
                .map_err(|e| type_error(format!("invalid argon2 parameters: {e}")))?;
            let salt = SaltString::encode_b64(salt)
                .map_err(|e| type_error(format!("invalid salt: {e}")))?;
            Argon2::new(variant, Version::V0x13, params)
                .hash_password(password, &salt)
                .map(|hash| hash.to_string())
                .map_err(|e| type_error(format!("argon2 hashing failed: {e}")))
        }
        "bcrypt" => {
            // 16 bytes exactly: bcrypt's salt is a fixed field of the `$2b$`
            // string, not a variable-length one, so a wrong length is the
            // caller's mistake rather than something to pad around.
            let salt: [u8; 16] = salt
                .try_into()
                .map_err(|_| type_error("bcrypt needs a 16-byte salt"))?;
            // `non_truncating_*`: bcrypt hashes at most 72 bytes *including*
            // the NUL it appends, so anything past 71 is ignored and the
            // truncating form would quietly make two different passwords the
            // same password. Refusing is the only answer that does not create a
            // silent equivalence class. (Verification stays truncating — see
            // `password_verify`.)
            bcrypt::non_truncating_hash_with_salt(password, params.cost, salt)
                .map(|parts| parts.to_string())
                .map_err(|e| type_error(format!("bcrypt hashing failed: {e}")))
        }
        "scrypt" => {
            use scrypt::Scrypt;
            use scrypt::password_hash::{PasswordHasher, SaltString};

            let params = scrypt::Params::new(
                params.cost as u8,
                params.block_size,
                params.parallelism,
                scrypt::Params::RECOMMENDED_LEN,
            )
            .map_err(|e| type_error(format!("invalid scrypt parameters: {e}")))?;
            let salt = SaltString::encode_b64(salt)
                .map_err(|e| type_error(format!("invalid salt: {e}")))?;
            Scrypt
                .hash_password_customized(password, None, None, params, &salt)
                .map(|hash| hash.to_string())
                .map_err(|e| type_error(format!("scrypt hashing failed: {e}")))
        }
        other => Err(type_error(format!(
            "unknown password algorithm '{other}' (expected 'argon2id', 'argon2i', 'argon2d', 'bcrypt', or 'scrypt')"
        ))),
    }
}

/// Verifies `password` against a stored hash, dispatching on what the string
/// says it is. The parameters come from the string too, so a hash written with
/// yesterday's cost still verifies after the default is raised — which is what
/// makes an unattended cost increase possible at all.
fn password_verify(password: &[u8], stored: &str) -> Result<bool, OpError> {
    // A `$2a$`/`$2b$`/`$2y$` prefix is bcrypt's own format, not PHC.
    if stored.starts_with("$2") {
        // The truncating form, deliberately: a stored hash may have been
        // written by an implementation that truncated (they nearly all do), so
        // verification has to compute what *that* implementation computed.
        return bcrypt::verify(password, stored)
            .map_err(|e| type_error(format!("bcrypt verification failed: {e}")));
    }

    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    let parsed = PasswordHash::new(stored)
        .map_err(|e| type_error(format!("unreadable password hash: {e}")))?;
    let algorithm = parsed.algorithm.as_str();
    let verified = match algorithm {
        "argon2id" | "argon2i" | "argon2d" => argon2::Argon2::default()
            .verify_password(password, &parsed)
            .is_ok(),
        "scrypt" => {
            // scrypt's `PasswordVerifier` reads its params from the same
            // string, but it is the 0.11 crate's own `password_hash`
            // re-export — the same 0.5 generation, so the parsed value passes
            // straight through.
            use scrypt::password_hash::PasswordVerifier as _;
            scrypt::Scrypt.verify_password(password, &parsed).is_ok()
        }
        other => {
            return Err(type_error(format!(
                "unsupported password hash algorithm '{other}'"
            )));
        }
    };
    Ok(verified)
}

// ---- installation ----------------------------------------------------------

/// Registers the `hash_*` ops. None takes a capability: every one is a pure
/// function of its arguments (see the module docs on salts).
pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    // Per-agent by construction: `install` runs once per `Runtime`, so the map
    // and its counter are this isolate's alone and an id from another agent
    // simply is not in it. Nothing here is provider-backed, so there is no
    // shared counter for `handles::Handles` to defend against (D50).
    let registry: Rc<RefCell<HashMap<u64, State>>> = Rc::new(RefCell::new(HashMap::new()));
    let next_id = Rc::new(RefCell::new(0u64));

    engine.register_op(OpDecl::sync("hash_digest", |mut args| {
        let alg = alg_arg(&args, 0)?;
        let encoding = encoding_arg(&args, 2)?;
        let data = bytes_arg(&mut args, 1)?;
        Ok(encoding.apply(alg.oneshot(&data)))
    }))?;

    let (reg, ids) = (registry.clone(), next_id.clone());
    engine.register_op(OpDecl::sync("hash_new", move |args| {
        let alg = alg_arg(&args, 0)?;
        let mut id = ids.borrow_mut();
        *id += 1;
        reg.borrow_mut().insert(*id, State::new(alg));
        Ok(Value::Number(*id as f64))
    }))?;

    let reg = registry.clone();
    engine.register_op(OpDecl::sync("hash_update", move |mut args| {
        let id = id_arg(&args, 0);
        let data = bytes_arg(&mut args, 1)?;
        let mut map = reg.borrow_mut();
        let state = map
            .get_mut(&id)
            .ok_or_else(|| type_error("this hasher has already produced its digest"))?;
        state.update(&data);
        Ok(Value::Undefined)
    }))?;

    let reg = registry.clone();
    engine.register_op(OpDecl::sync("hash_finish", move |args| {
        let id = id_arg(&args, 0);
        let encoding = encoding_arg(&args, 1)?;
        let state = reg
            .borrow_mut()
            .remove(&id)
            .ok_or_else(|| type_error("this hasher has already produced its digest"))?;
        Ok(encoding.apply(state.finish()))
    }))?;

    // Called from the module's `FinalizationRegistry`, for a hasher dropped
    // without ever being digested. `hash_finish` is the ordinary end of life;
    // this is the one that keeps an abandoned hasher from holding its state for
    // the life of the isolate.
    engine.register_op(OpDecl::sync("hash_free", move |args| {
        registry.borrow_mut().remove(&id_arg(&args, 0));
        Ok(Value::Undefined)
    }))?;

    engine.register_op(OpDecl::sync("hash_hmac", |mut args| {
        let alg = alg_arg(&args, 0)?;
        let encoding = encoding_arg(&args, 3)?;
        let key = bytes_arg(&mut args, 1)?;
        let data = bytes_arg(&mut args, 2)?;
        Ok(encoding.apply(hmac_sign(alg, &key, &data)?))
    }))?;

    engine.register_op(OpDecl::sync("hash_password", |mut args| {
        let algorithm = args
            .first()
            .and_then(Value::as_str)
            .unwrap_or("argon2id")
            .to_owned();
        let fields = match args.get(3) {
            Some(Value::Object(fields)) => fields.clone(),
            _ => Vec::new(),
        };
        let params = PwParams {
            memory: u32_field(&fields, "memoryCost", 19 * 1024),
            time: u32_field(&fields, "timeCost", 2),
            parallelism: u32_field(&fields, "parallelism", 1),
            cost: u32_field(&fields, "cost", 12),
            block_size: u32_field(&fields, "blockSize", 8),
        };
        let password = bytes_arg(&mut args, 1)?;
        let salt = bytes_arg(&mut args, 2)?;
        Ok(Value::String(password_hash(
            &algorithm, &password, &salt, params,
        )?))
    }))?;

    engine.register_op(OpDecl::sync("hash_password_verify", |mut args| {
        let stored = args
            .get(1)
            .and_then(Value::as_str)
            .ok_or_else(|| type_error("a stored password hash must be a string"))?
            .to_owned();
        let password = bytes_arg(&mut args, 0)?;
        Ok(Value::Bool(password_verify(&password, &stored)?))
    }))?;

    engine.register_op(OpDecl::sync("hash_equal", |mut args| {
        let a = bytes_arg(&mut args, 0)?;
        let b = bytes_arg(&mut args, 1)?;
        // Length is compared first and in variable time, which leaks only the
        // length — already public for a digest, whose size its algorithm
        // fixes. The contents are then compared in constant time.
        if a.len() != b.len() {
            return Ok(Value::Bool(false));
        }
        Ok(Value::Bool(bool::from(a.ct_eq(&b))))
    }))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        match Encoding::Hex.apply(bytes.to_vec()) {
            Value::String(s) => s,
            other => panic!("expected a string, got {other:?}"),
        }
    }

    fn digest_hex(name: &str, data: &[u8]) -> String {
        hex(&Alg::parse(name).expect("known algorithm").oneshot(data))
    }

    // Published vectors, one per algorithm: NIST's for the SHA families, RFC
    // 1321 for MD5, the RIPEMD-160 reference for that, BLAKE3's own test
    // vectors, xxHash's, and the classic "123456789" check value for both CRCs.
    #[test]
    fn every_algorithm_matches_its_published_vector() {
        assert_eq!(
            digest_hex("sha1", b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            digest_hex("sha256", b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest_hex("sha384", b"abc"),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed\
             8086072ba1e7cc2358baeca134c825a7"
        );
        assert_eq!(
            digest_hex("sha512", b"abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        );
        assert_eq!(
            digest_hex("sha3-224", b"abc"),
            "e642824c3f8cf24ad09234ee7d3c766fc9a3a5168d0c94ad73b46fdf"
        );
        assert_eq!(
            digest_hex("sha3-256", b"abc"),
            "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532"
        );
        assert_eq!(
            digest_hex("sha3-384", b"abc"),
            "ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c2596da7cf0e49be4b2\
             98d88cea927ac7f539f1edf228376d25"
        );
        assert_eq!(
            digest_hex("sha3-512", b"abc"),
            "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e\
             10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0"
        );
        assert_eq!(
            digest_hex("blake3", b"abc"),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
        assert_eq!(
            digest_hex("md5", b"abc"),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(
            digest_hex("ripemd160", b"abc"),
            "8eb208f7e05d987a9b044a8e98c6b087f15a0bfc"
        );
        assert_eq!(digest_hex("xxhash64", b""), "ef46db3751d8e999");
        assert_eq!(digest_hex("xxhash3", b""), "2d06800538d394c2");
        assert_eq!(digest_hex("crc32", b"123456789"), "cbf43926");
        assert_eq!(digest_hex("crc32c", b"123456789"), "e3069283");
    }

    /// The whole reason the incremental path exists: it must agree with the
    /// one-shot path for every algorithm, at a chunk boundary that falls inside
    /// each one's block.
    #[test]
    fn incremental_agrees_with_one_shot_for_every_algorithm() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        for (name, alg) in Alg::ALL {
            let mut state = State::new(alg);
            for chunk in data.chunks(7) {
                state.update(chunk);
            }
            assert_eq!(
                hex(&state.finish()),
                hex(&alg.oneshot(&data)),
                "{name} disagrees with itself across chunks"
            );
        }
    }

    #[test]
    fn an_empty_input_is_hashable_by_every_algorithm() {
        for (name, alg) in Alg::ALL {
            let one_shot = alg.oneshot(b"");
            assert!(!one_shot.is_empty(), "{name} produced no digest");
            assert_eq!(hex(&State::new(alg).finish()), hex(&one_shot), "{name}");
        }
    }

    #[test]
    fn webcrypto_spellings_and_case_both_parse() {
        assert_eq!(Alg::parse("SHA-256"), Some(Alg::Sha256));
        assert_eq!(Alg::parse("sha-256"), Some(Alg::Sha256));
        assert_eq!(Alg::parse("SHA256"), Some(Alg::Sha256));
        assert_eq!(Alg::parse("SHA3-512"), Some(Alg::Sha3_512));
        assert_eq!(Alg::parse("BLAKE3"), Some(Alg::Blake3));
        assert_eq!(Alg::parse("CRC32C"), Some(Alg::Crc32c));
        assert_eq!(Alg::parse("sha3512"), None);
        assert_eq!(Alg::parse("sha2"), None);
        assert_eq!(Alg::parse(""), None);
    }

    #[test]
    fn the_encodings_encode_the_same_digest() {
        let digest = Alg::Sha256.oneshot(b"abc");
        assert_eq!(
            Encoding::Hex.apply(digest.clone()),
            Value::String(
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into()
            )
        );
        assert_eq!(
            Encoding::Base64.apply(digest.clone()),
            Value::String("ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=".into())
        );
        assert_eq!(
            Encoding::Base64Url.apply(digest.clone()),
            Value::String("ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0".into())
        );
        assert_eq!(Encoding::Bytes.apply(digest.clone()), Value::Bytes(digest));
        assert!(Encoding::parse(Some("base32")).is_none());
    }

    /// RFC 2202 / RFC 4231 test case 2 ("Jefe" / "what do ya want for nothing?").
    #[test]
    fn hmac_matches_its_rfc_vectors() {
        let (key, data) = (
            b"Jefe".as_slice(),
            b"what do ya want for nothing?".as_slice(),
        );
        assert_eq!(
            hex(&hmac_sign(Alg::Sha1, key, data).unwrap()),
            "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79"
        );
        assert_eq!(
            hex(&hmac_sign(Alg::Sha256, key, data).unwrap()),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
        assert_eq!(
            hex(&hmac_sign(Alg::Md5, key, data).unwrap()),
            "750c783e6ab0b503eaa86e310a5db738"
        );
    }

    #[test]
    fn hmac_refuses_a_checksum_and_blake3() {
        for alg in [Alg::Crc32, Alg::Crc32c, Alg::XxHash64, Alg::XxHash3] {
            assert!(hmac_sign(alg, b"k", b"d").is_err(), "{alg:?} was accepted");
        }
        let err = hmac_sign(Alg::Blake3, b"k", b"d").unwrap_err().to_string();
        assert!(err.contains("keyed mode"), "{err}");
    }

    /// Cheap parameters throughout — these test the plumbing (a hash verifies,
    /// a wrong password does not), not the KDFs' own correctness.
    fn cheap(algorithm: &str) -> PwParams {
        let _ = algorithm;
        PwParams {
            memory: 32,
            time: 1,
            parallelism: 1,
            cost: 4,
            block_size: 8,
        }
    }

    #[test]
    fn every_password_algorithm_round_trips() {
        for algorithm in ["argon2id", "argon2i", "argon2d", "bcrypt", "scrypt"] {
            let stored =
                password_hash(algorithm, b"correct horse", &[7u8; 16], cheap(algorithm)).unwrap();
            assert!(
                password_verify(b"correct horse", &stored).unwrap(),
                "{algorithm} did not verify its own hash: {stored}"
            );
            assert!(
                !password_verify(b"Correct horse", &stored).unwrap(),
                "{algorithm} accepted the wrong password"
            );
        }
    }

    #[test]
    fn a_password_hash_names_its_algorithm_and_carries_its_salt() {
        let argon = password_hash("argon2id", b"pw", &[1u8; 16], cheap("argon2id")).unwrap();
        assert!(argon.starts_with("$argon2id$"), "{argon}");
        let bcrypt = password_hash("bcrypt", b"pw", &[1u8; 16], cheap("bcrypt")).unwrap();
        assert!(bcrypt.starts_with("$2b$04$"), "{bcrypt}");
        let scrypt = password_hash("scrypt", b"pw", &[1u8; 16], cheap("scrypt")).unwrap();
        assert!(scrypt.starts_with("$scrypt$"), "{scrypt}");

        // Two hashes of one password differ, because the salt differs — the
        // property the whole salt argument exists for.
        let a = password_hash("argon2id", b"pw", &[1u8; 16], cheap("argon2id")).unwrap();
        let b = password_hash("argon2id", b"pw", &[2u8; 16], cheap("argon2id")).unwrap();
        assert_ne!(a, b);
    }

    /// A hash written with weaker parameters must still verify after the
    /// defaults are raised: the parameters are read from the string, never
    /// from today's configuration.
    #[test]
    fn a_hash_verifies_against_the_parameters_it_was_written_with() {
        let weak = password_hash(
            "argon2id",
            b"pw",
            &[3u8; 16],
            PwParams {
                memory: 32,
                time: 1,
                parallelism: 1,
                cost: 4,
                block_size: 8,
            },
        )
        .unwrap();
        assert!(weak.contains("m=32,t=1,p=1"), "{weak}");
        assert!(password_verify(b"pw", &weak).unwrap());
    }

    #[test]
    fn bcrypt_refuses_a_password_it_would_have_truncated() {
        let long = vec![b'a'; 73];
        let err = password_hash("bcrypt", &long, &[0u8; 16], cheap("bcrypt")).unwrap_err();
        assert!(err.to_string().contains("bcrypt"), "{err}");
        // 71 is the boundary: the 72-byte budget includes the NUL bcrypt
        // appends, so a 72-byte password already loses its last byte.
        assert!(password_hash("bcrypt", &long[..71], &[0u8; 16], cheap("bcrypt")).is_ok());
        assert!(password_hash("bcrypt", &long[..72], &[0u8; 16], cheap("bcrypt")).is_err());
    }

    #[test]
    fn bcrypt_needs_exactly_sixteen_salt_bytes() {
        let err = password_hash("bcrypt", b"pw", &[0u8; 8], cheap("bcrypt")).unwrap_err();
        assert!(err.to_string().contains("16-byte salt"), "{err}");
    }

    /// A `$2a$` hash — the version Node's `bcrypt`/`bcryptjs` write — must
    /// verify, or migrating onto this runtime would lock every existing user
    /// out. Only the version tag differs from what we write, so the same digest
    /// is checked under both prefixes.
    #[test]
    fn a_bcrypt_hash_written_as_2a_still_verifies() {
        let parts = bcrypt::hash_with_salt(b"correct horse", 4, [9u8; 16]).unwrap();
        let ours = parts.to_string();
        let foreign = parts.format_for_version(bcrypt::Version::TwoA);
        assert!(ours.starts_with("$2b$"), "{ours}");
        assert!(foreign.starts_with("$2a$"), "{foreign}");
        assert!(password_verify(b"correct horse", &foreign).unwrap());
        assert!(!password_verify(b"wrong horse", &foreign).unwrap());
    }

    #[test]
    fn an_unreadable_or_unknown_stored_hash_is_refused() {
        assert!(password_verify(b"pw", "not a hash at all").is_err());
        assert!(password_verify(b"pw", "$pbkdf2-sha256$i=1000$c2FsdA$aGFzaA").is_err());
        assert!(password_hash("md5", b"pw", &[0u8; 16], cheap("md5")).is_err());
    }
}
