//! Host ops behind signed and sealed cookies in `runtime:http` (DECISIONS D161).
//!
//! Pure computation and ungated, like `hashing_ops`: the nonce a seal needs is
//! drawn in JS through `crypto.getRandomValues` and passed in, so sealing
//! inherits the Entropy gate and these stay functions of their arguments.
//!
//! Each secret is stretched with HKDF-SHA256 into a signing key and a sealing
//! key, so one secret can serve both without the two uses sharing a key. The
//! cookie name and the expiry are always inside what is authenticated: a value
//! cannot be moved to another cookie, and its expiry cannot be edited.

use aes_gcm::aead::{Aead, KeyInit as _, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

/// The shortest secret accepted, in bytes: 256 bits, the key size behind it.
pub(crate) const MIN_SECRET: usize = 32;
const NONCE: usize = 12;

/// Distinct HKDF `info` strings, so the signing and sealing keys from one
/// secret are independent. The version is part of the format.
const SIGN_INFO: &[u8] = b"esrun cookie hmac-sha256 v1";
const SEAL_INFO: &[u8] = b"esrun cookie aes-256-gcm v1";

fn derive(secret: &[u8], info: &[u8]) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(None, secret)
        .expand(info, &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    key
}

/// The HMAC tag over the name, expiry and value, base64url.
pub(crate) fn sign(secret: &[u8], name: &str, expiry: u64, value: &str) -> String {
    URL_SAFE_NO_PAD.encode(tag(secret, name, expiry, value))
}

fn tag(secret: &[u8], name: &str, expiry: u64, value: &str) -> Vec<u8> {
    let mut mac =
        <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(&derive(secret, SIGN_INFO))
            .expect("HMAC accepts any key length");
    // Lengths before the parts, so no two (name, expiry, value) triples share
    // an encoding.
    for part in [
        name.as_bytes(),
        expiry.to_string().as_bytes(),
        value.as_bytes(),
    ] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part);
    }
    mac.finalize().into_bytes().to_vec()
}

/// Whether `tag` is the tag of (name, expiry, value) under any of `secrets`.
/// Every secret is tried, so the time taken does not say which one matched.
pub(crate) fn verify(
    secrets: &[Vec<u8>],
    name: &str,
    expiry: u64,
    value: &str,
    tag_text: &str,
) -> bool {
    let Ok(given) = URL_SAFE_NO_PAD.decode(tag_text) else {
        return false;
    };
    let mut ok = 0u8;
    for secret in secrets {
        ok |= tag(secret, name, expiry, value).ct_eq(&given).unwrap_u8();
    }
    ok == 1
}

fn aad(name: &str) -> Vec<u8> {
    let mut aad = SEAL_INFO.to_vec();
    aad.push(0);
    aad.extend_from_slice(name.as_bytes());
    aad
}

/// Seals (expiry, value) for cookie `name`: base64url of nonce ‖ ciphertext ‖ tag.
pub(crate) fn seal(
    secret: &[u8],
    name: &str,
    expiry: u64,
    value: &str,
    nonce: &[u8; NONCE],
) -> String {
    let cipher = Aes256Gcm::new_from_slice(&derive(secret, SEAL_INFO)).expect("a 32-byte key");
    let mut plain = expiry.to_be_bytes().to_vec();
    plain.extend_from_slice(value.as_bytes());
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: &plain,
                aad: &aad(name),
            },
        )
        .expect("AES-GCM encryption of a cookie-sized message cannot fail");
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    URL_SAFE_NO_PAD.encode(out)
}

/// Opens a sealed cookie with the first of `secrets` that authenticates it.
/// `None` for anything that is not a seal of a UTF-8 value under this name.
pub(crate) fn open(secrets: &[Vec<u8>], name: &str, text: &str) -> Option<(u64, String)> {
    let bytes = URL_SAFE_NO_PAD.decode(text).ok()?;
    if bytes.len() < NONCE {
        return None;
    }
    let (nonce, sealed) = bytes.split_at(NONCE);
    let aad = aad(name);
    secrets.iter().find_map(|secret| {
        let cipher = Aes256Gcm::new_from_slice(&derive(secret, SEAL_INFO)).ok()?;
        let plain = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .ok()?;
        let (expiry, value) = plain.split_at_checked(8)?;
        let expiry = u64::from_be_bytes(expiry.try_into().ok()?);
        Some((expiry, String::from_utf8(value.to_vec()).ok()?))
    })
}

fn type_error(message: &str) -> OpError {
    OpError::type_error(message.to_string())
}

fn str_arg(args: &[Value], index: usize) -> Result<&str, OpError> {
    args.get(index)
        .and_then(Value::as_str)
        .ok_or_else(|| type_error("expected a string"))
}

fn expiry_arg(args: &[Value], index: usize) -> u64 {
    args.get(index)
        .and_then(Value::as_number)
        .unwrap_or(0.0)
        .max(0.0) as u64
}

/// The secrets, already checked and encoded to bytes in JS.
fn secrets_arg(args: &mut [Value], index: usize) -> Result<Vec<Vec<u8>>, OpError> {
    let Some(Value::Array(items)) = args
        .get_mut(index)
        .map(|slot| std::mem::replace(slot, Value::Undefined))
    else {
        return Err(type_error("expected an array of secrets"));
    };
    items
        .into_iter()
        .map(|item| {
            item.into_bytes()
                .filter(|b| b.len() >= MIN_SECRET)
                .ok_or_else(|| type_error("each secret must be at least 32 bytes"))
        })
        .collect()
}

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    // (secrets, name, expiry, value) → tag, signed with the first secret.
    engine.register_op(OpDecl::sync("cookie_sign", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        let first = secrets
            .first()
            .ok_or_else(|| type_error("secrets is empty"))?;
        Ok(Value::String(sign(
            first,
            str_arg(&args, 1)?,
            expiry_arg(&args, 2),
            str_arg(&args, 3)?,
        )))
    }))?;
    // (secrets, name, expiry, value, tag) → boolean.
    engine.register_op(OpDecl::sync("cookie_verify", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        Ok(Value::Bool(verify(
            &secrets,
            str_arg(&args, 1)?,
            expiry_arg(&args, 2),
            str_arg(&args, 3)?,
            str_arg(&args, 4)?,
        )))
    }))?;
    // (secrets, name, expiry, value, nonce) → sealed text, with the first secret.
    engine.register_op(OpDecl::sync("cookie_seal", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        let first = secrets
            .first()
            .ok_or_else(|| type_error("secrets is empty"))?;
        let nonce: [u8; NONCE] = args
            .get_mut(4)
            .map(|slot| std::mem::replace(slot, Value::Undefined))
            .and_then(Value::into_bytes)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| type_error("expected a 12-byte nonce"))?;
        Ok(Value::String(seal(
            first,
            str_arg(&args, 1)?,
            expiry_arg(&args, 2),
            str_arg(&args, 3)?,
            &nonce,
        )))
    }))?;
    // (secrets, name, text) → [expiry, value] or null.
    engine.register_op(OpDecl::sync("cookie_open", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        Ok(
            match open(&secrets, str_arg(&args, 1)?, str_arg(&args, 2)?) {
                Some((expiry, value)) => {
                    Value::Array(vec![Value::Number(expiry as f64), Value::String(value)])
                }
                None => Value::Null,
            },
        )
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &[u8] = b"0123456789abcdef0123456789abcdef";
    const B: &[u8] = b"fedcba9876543210fedcba9876543210";

    #[test]
    fn a_tag_verifies_under_its_secret_and_no_other() {
        let tag = sign(A, "sid", 0, "user-42");
        assert_eq!(tag.len(), 43);
        assert!(verify(&[A.to_vec()], "sid", 0, "user-42", &tag));
        // Rotation: the old secret still verifies when listed after the new one.
        assert!(verify(&[B.to_vec(), A.to_vec()], "sid", 0, "user-42", &tag));
        assert!(!verify(&[B.to_vec()], "sid", 0, "user-42", &tag));
    }

    #[test]
    fn a_tag_binds_the_name_the_expiry_and_the_value() {
        let tag = sign(A, "sid", 100, "v");
        let keys = [A.to_vec()];
        assert!(!verify(&keys, "other", 100, "v", &tag));
        assert!(!verify(&keys, "sid", 101, "v", &tag));
        assert!(!verify(&keys, "sid", 100, "w", &tag));
        assert!(!verify(&keys, "sid", 100, "v", "not base64!"));
        // Length prefixes: moving a byte between parts changes the tag.
        assert_ne!(sign(A, "ab", 1, "c"), sign(A, "a", 1, "bc"));
    }

    #[test]
    fn a_seal_opens_under_its_secret_and_name_only() {
        let nonce = [7u8; NONCE];
        let text = seal(A, "session", 1_900_000_000, "{\"u\":1}", &nonce);
        assert_eq!(
            open(&[A.to_vec()], "session", &text),
            Some((1_900_000_000, "{\"u\":1}".to_string()))
        );
        assert_eq!(
            open(&[B.to_vec(), A.to_vec()], "session", &text).map(|o| o.1),
            Some("{\"u\":1}".to_string())
        );
        assert_eq!(open(&[B.to_vec()], "session", &text), None);
        assert_eq!(open(&[A.to_vec()], "other", &text), None);
        // Any changed byte fails authentication.
        let mut bytes = URL_SAFE_NO_PAD.decode(&text).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert_eq!(
            open(&[A.to_vec()], "session", &URL_SAFE_NO_PAD.encode(bytes)),
            None
        );
        assert_eq!(open(&[A.to_vec()], "session", "short"), None);
        assert_eq!(open(&[A.to_vec()], "session", "!!!"), None);
    }

    #[test]
    fn the_sealed_value_is_not_readable() {
        let text = seal(A, "s", 0, "plain-secret-text", &[1u8; NONCE]);
        let bytes = URL_SAFE_NO_PAD.decode(&text).unwrap();
        assert!(!bytes.windows(5).any(|w| w == b"plain"));
        // nonce + 8-byte expiry + value + 16-byte tag.
        assert_eq!(bytes.len(), NONCE + 8 + 17 + 16);
    }
}
