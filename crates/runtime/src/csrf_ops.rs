//! Host ops behind `CSRF` in `runtime:http` (DECISIONS D162).
//!
//! A token is base64url of nonce ‖ issued ‖ lifetime ‖ tag: a 16-byte random
//! nonce, the issue time and the lifetime as big-endian milliseconds, and an
//! HMAC-SHA256 over the session ID and those three. Nothing is stored; the
//! server recomputes the tag. The key comes from the secret through HKDF with
//! an `info` of its own, so a CSRF key never equals a cookie key (D161).
//!
//! Pure functions of their arguments: the nonce and the clock reading come in
//! from JS, so drawing the nonce passes the Entropy gate there.

use crate::cookie_ops::{derive, secrets_arg, str_arg, type_error};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use es_runtime_engine::{Engine, OpDecl, Value};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

const INFO: &[u8] = b"esrun csrf hmac-sha256 v1";
const NONCE: usize = 16;
const TAG: usize = 32;
const TOKEN: usize = NONCE + 8 + 8 + TAG;

/// How far in the future an issue time may be, for clocks that differ
/// between the servers that issue and check a token.
const CLOCK_SKEW_MS: u64 = 60_000;

fn tag(secret: &[u8], session: &str, body: &[u8]) -> [u8; TAG] {
    let mut mac = <Hmac<Sha256> as hmac::digest::KeyInit>::new_from_slice(&derive(secret, INFO))
        .expect("HMAC accepts any key length");
    // The session ID is length-prefixed; the body is fixed-size.
    mac.update(&(session.len() as u64).to_be_bytes());
    mac.update(session.as_bytes());
    mac.update(body);
    mac.finalize().into_bytes().into()
}

pub(crate) fn generate(
    secret: &[u8],
    session: &str,
    issued_ms: u64,
    lifetime_ms: u64,
    nonce: &[u8; NONCE],
) -> String {
    let mut token = Vec::with_capacity(TOKEN);
    token.extend_from_slice(nonce);
    token.extend_from_slice(&issued_ms.to_be_bytes());
    token.extend_from_slice(&lifetime_ms.to_be_bytes());
    let tag = tag(secret, session, &token);
    token.extend_from_slice(&tag);
    URL_SAFE_NO_PAD.encode(token)
}

/// Whether `token` was issued for `session` under one of `secrets` and is
/// still within both its own lifetime and `max_age_ms`.
pub(crate) fn verify(
    secrets: &[Vec<u8>],
    session: &str,
    token: &str,
    now_ms: u64,
    max_age_ms: u64,
) -> bool {
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(token) else {
        return false;
    };
    if bytes.len() != TOKEN {
        return false;
    }
    let (body, given) = bytes.split_at(TOKEN - TAG);
    // Every secret is tried, so the time taken does not say which matched.
    let mut ok = 0u8;
    for secret in secrets {
        ok |= tag(secret, session, body).ct_eq(given).unwrap_u8();
    }
    if ok != 1 {
        return false;
    }
    let issued = u64::from_be_bytes(body[NONCE..NONCE + 8].try_into().unwrap());
    let lifetime = u64::from_be_bytes(body[NONCE + 8..].try_into().unwrap());
    if issued > now_ms.saturating_add(CLOCK_SKEW_MS) {
        return false;
    }
    let age = now_ms.saturating_sub(issued);
    age <= max_age_ms && age <= lifetime
}

fn ms_arg(args: &[Value], index: usize) -> u64 {
    args.get(index)
        .and_then(Value::as_number)
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
        .max(0.0) as u64
}

pub(crate) fn install(engine: &mut dyn Engine) -> crate::Result<()> {
    // (secrets, sessionId, issuedMs, lifetimeMs, nonce) → token, with the first secret.
    engine.register_op(OpDecl::sync("csrf_generate", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        let first = secrets
            .first()
            .ok_or_else(|| type_error("secrets is empty"))?;
        let nonce: [u8; NONCE] = args
            .get_mut(4)
            .map(|slot| std::mem::replace(slot, Value::Undefined))
            .and_then(Value::into_bytes)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| type_error("expected a 16-byte nonce"))?;
        Ok(Value::String(generate(
            first,
            str_arg(&args, 1)?,
            ms_arg(&args, 2),
            ms_arg(&args, 3),
            &nonce,
        )))
    }))?;
    // (secrets, sessionId, token, nowMs, maxAgeMs) → boolean.
    engine.register_op(OpDecl::sync("csrf_verify", |mut args| {
        let secrets = secrets_arg(&mut args, 0)?;
        Ok(Value::Bool(verify(
            &secrets,
            str_arg(&args, 1)?,
            str_arg(&args, 2)?,
            ms_arg(&args, 3),
            ms_arg(&args, 4),
        )))
    }))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &[u8] = b"0123456789abcdef0123456789abcdef";
    const B: &[u8] = b"fedcba9876543210fedcba9876543210";
    const DAY: u64 = 86_400_000;
    const T0: u64 = 1_800_000_000_000;

    fn token() -> String {
        generate(A, "sess-1", T0, DAY, &[9u8; NONCE])
    }

    #[test]
    fn a_token_verifies_for_its_session_under_its_secret() {
        let t = token();
        assert_eq!(t.len(), 86);
        assert!(verify(&[A.to_vec()], "sess-1", &t, T0 + 1000, DAY));
        assert!(verify(&[B.to_vec(), A.to_vec()], "sess-1", &t, T0, DAY));
        assert!(!verify(&[B.to_vec()], "sess-1", &t, T0, DAY));
        assert!(!verify(&[A.to_vec()], "sess-2", &t, T0, DAY));
        assert!(!verify(&[A.to_vec()], "", &t, T0, DAY));
    }

    #[test]
    fn a_changed_or_malformed_token_fails() {
        let mut bytes = URL_SAFE_NO_PAD.decode(token()).unwrap();
        // Extending the lifetime is a change to the signed body.
        bytes[NONCE + 15] = 0xff;
        assert!(!verify(
            &[A.to_vec()],
            "sess-1",
            &URL_SAFE_NO_PAD.encode(&bytes),
            T0,
            DAY
        ));
        for bad in ["", "x", "!!!!", &token()[..85], &format!("{}A", token())] {
            assert!(!verify(&[A.to_vec()], "sess-1", bad, T0, DAY), "{bad}");
        }
    }

    #[test]
    fn age_is_bounded_by_the_lifetime_and_max_age_and_the_clock() {
        let t = token();
        let keys = [A.to_vec()];
        assert!(verify(&keys, "sess-1", &t, T0 + DAY, DAY));
        assert!(!verify(&keys, "sess-1", &t, T0 + DAY + 1, DAY));
        // A shorter maxAge at the verifier wins over the token's own lifetime.
        assert!(!verify(&keys, "sess-1", &t, T0 + 61_000, 60_000));
        // Issued slightly ahead of this server's clock: accepted within the skew.
        assert!(verify(&keys, "sess-1", &t, T0 - CLOCK_SKEW_MS, DAY));
        assert!(!verify(&keys, "sess-1", &t, T0 - CLOCK_SKEW_MS - 1, DAY));
    }

    #[test]
    fn the_session_id_cannot_shift_into_the_body() {
        // Length-prefixed: no two sessions share a MAC input.
        assert_ne!(tag(A, "ab", b"c"), tag(A, "a", b"bc"));
    }
}
