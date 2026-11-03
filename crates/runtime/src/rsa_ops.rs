//! Host ops backing RSA WebCrypto: RSASSA-PKCS1-v1_5, RSA-PSS (sign/verify),
//! and RSA-OAEP (encrypt/decrypt), via the RustCrypto `rsa` crate
//! (DECISIONS.md D9). Closes out the Phase 7b `crypto.subtle` surface.
//!
//! Keys cross the op boundary as PKCS#8 DER (private) / SPKI DER (public). JWK
//! components (`n`/`e` and the private CRT params `d`/`p`/`q`/`dp`/`dq`/`qi`)
//! are exchanged via a small length-prefixed framing ([`frame`]) and assembled
//! into JWK objects by the prelude.
//!
//! Randomness — RSA key generation, PSS salts, OAEP padding, and the blinding
//! of every private-key operation (signing and decryption) — is drawn from the
//! injected [`Entropy`] provider (via the shared [`crate::ec_ops::EntropyRng`]
//! adapter), never ambient `OsRng`: the crate's `getrandom` feature is off, so
//! each private-key call here passes the RNG explicitly (D203). The message
//! prehash honours an arbitrary `algorithm.hash`, computed with our `sha2`
//! 0.11 ([`crate::ec_ops::prehash`]), and the DigestInfo prefix uses the same
//! `sha1`/`sha2` 0.11 types.
//!
//! `rsa` 0.10 does its private-key arithmetic in constant time
//! (crypto-bigint), blinds it, and checks padding without branching on secret
//! data — the fixes for RUSTSEC-2023-0071. Secret components are therefore
//! decoded at a fixed width taken from the public modulus, never sized from
//! their own bytes.

use std::sync::Arc;

use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::Entropy;

use rsa::pkcs8::{DecodePrivateKey, DecodePublicKey, EncodePrivateKey, EncodePublicKey};
use rsa::traits::{PrivateKeyParts, PublicKeyParts};
use rsa::{BoxedUint, Oaep, Pkcs1v15Sign, Pss, RsaPrivateKey, RsaPublicKey};

use crate::Result;
use crate::crypto_ops::{arg_bytes, arg_str, data_error, not_supported, operation_error};
use crate::ec_ops::{EntropyRng, prehash};

/// Binds a hash name to the matching `digest` 0.11 type (with `AssociatedOid`)
/// the `rsa` padding schemes need for their DigestInfo prefix, then evaluates
/// `$body` with that type bound to `$d`. Unknown hashes `return` an error from
/// the enclosing function.
macro_rules! with_oid_digest {
    ($hash:expr, $d:ident => $body:expr) => {
        match $hash {
            "SHA-1" => {
                type $d = sha1::Sha1;
                $body
            }
            "SHA-256" => {
                type $d = sha2::Sha256;
                $body
            }
            "SHA-384" => {
                type $d = sha2::Sha384;
                $body
            }
            "SHA-512" => {
                type $d = sha2::Sha512;
                $body
            }
            other => return Err(not_supported(format!("unsupported RSA hash: {other}"))),
        }
    };
}

/// Length-prefixed framing (`u32` big-endian length + bytes, repeated) so a
/// single `Value::Bytes` can carry several big-integer components.
fn frame(parts: &[&[u8]]) -> Vec<u8> {
    let mut out = Vec::new();
    for part in parts {
        out.extend_from_slice(&(part.len() as u32).to_be_bytes());
        out.extend_from_slice(part);
    }
    out
}

/// Registers the RSA host ops. Several need entropy (key gen, PSS salt, PKCS#1
/// blinding, OAEP padding), so the provider is cloned per op.
pub(crate) fn install(engine: &mut dyn Engine, entropy: Arc<dyn Entropy>) -> Result<()> {
    let gen_entropy = entropy.clone();
    engine.register_op(OpDecl::sync("rsa_generate_pkcs8", move |args| {
        let bits = args.first().and_then(Value::as_number).unwrap_or(0.0) as usize;
        let exp = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(generate(gen_entropy.as_ref(), bits, &exp)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_public_spki", |args| {
        Ok(Value::Bytes(public_spki(&arg_bytes(&args, 0)?)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_import_pkcs8", |args| {
        Ok(Value::Bytes(import_pkcs8(&arg_bytes(&args, 0)?)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_import_spki", |args| {
        Ok(Value::Bytes(import_spki(&arg_bytes(&args, 0)?)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_jwk_public_params", |args| {
        Ok(Value::Bytes(jwk_public_params(&arg_bytes(&args, 0)?)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_jwk_private_params", |args| {
        Ok(Value::Bytes(jwk_private_params(&arg_bytes(&args, 0)?)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_spki_from_jwk", |args| {
        let n = arg_bytes(&args, 0)?;
        let e = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(spki_from_jwk(&n, &e)?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_pkcs8_from_jwk", |args| {
        let n = arg_bytes(&args, 0)?;
        let e = arg_bytes(&args, 1)?;
        let d = arg_bytes(&args, 2)?;
        let p = arg_bytes(&args, 3)?;
        let q = arg_bytes(&args, 4)?;
        Ok(Value::Bytes(pkcs8_from_jwk(&n, &e, &d, &p, &q)?))
    }))?;

    let sign_entropy = entropy.clone();
    engine.register_op(OpDecl::sync("rsa_pkcs1v15_sign", move |args| {
        let hash = arg_str(&args, 0)?;
        let pkcs8 = arg_bytes(&args, 1)?;
        let data = arg_bytes(&args, 2)?;
        Ok(Value::Bytes(pkcs1v15_sign(
            sign_entropy.as_ref(),
            &hash,
            &pkcs8,
            &data,
        )?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_pkcs1v15_verify", |args| {
        let hash = arg_str(&args, 0)?;
        let spki = arg_bytes(&args, 1)?;
        let signature = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bool(pkcs1v15_verify(
            &hash, &spki, &signature, &data,
        )?))
    }))?;

    let pss_entropy = entropy.clone();
    engine.register_op(OpDecl::sync("rsa_pss_sign", move |args| {
        let hash = arg_str(&args, 0)?;
        let salt_len = args.get(1).and_then(Value::as_number).unwrap_or(0.0) as usize;
        let pkcs8 = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bytes(pss_sign(
            pss_entropy.as_ref(),
            &hash,
            salt_len,
            &pkcs8,
            &data,
        )?))
    }))?;

    engine.register_op(OpDecl::sync("rsa_pss_verify", |args| {
        let hash = arg_str(&args, 0)?;
        let salt_len = args.get(1).and_then(Value::as_number).unwrap_or(0.0) as usize;
        let spki = arg_bytes(&args, 2)?;
        let signature = arg_bytes(&args, 3)?;
        let data = arg_bytes(&args, 4)?;
        Ok(Value::Bool(pss_verify(
            &hash, salt_len, &spki, &signature, &data,
        )?))
    }))?;

    let oaep_entropy = entropy.clone();
    engine.register_op(OpDecl::sync("rsa_oaep_encrypt", move |args| {
        let hash = arg_str(&args, 0)?;
        let label = arg_bytes(&args, 1)?;
        let spki = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bytes(oaep_encrypt(
            oaep_entropy.as_ref(),
            &hash,
            &label,
            &spki,
            &data,
        )?))
    }))?;

    let decrypt_entropy = entropy;
    engine.register_op(OpDecl::sync("rsa_oaep_decrypt", move |args| {
        let hash = arg_str(&args, 0)?;
        let label = arg_bytes(&args, 1)?;
        let pkcs8 = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        Ok(Value::Bytes(oaep_decrypt(
            decrypt_entropy.as_ref(),
            &hash,
            &label,
            &pkcs8,
            &data,
        )?))
    }))?;

    Ok(())
}

fn generate(
    entropy: &dyn Entropy,
    bits: usize,
    exp: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    if !(RsaPublicKey::MIN_SIZE..=RsaPublicKey::MAX_SIZE).contains(&bits) {
        return Err(operation_error(format!(
            "RSA modulusLength must be between {} and {} bits",
            RsaPublicKey::MIN_SIZE,
            RsaPublicKey::MAX_SIZE
        )));
    }
    let e = BoxedUint::from_be_slice_vartime(exp);
    let mut rng = EntropyRng {
        entropy,
        failed: false,
    };
    let key = RsaPrivateKey::new_with_exp(&mut rng, bits, e)
        .map_err(|_| operation_error("RSA key generation failed"))?;
    if rng.failed {
        return Err(operation_error(
            "entropy provider failed during key generation",
        ));
    }
    encode_pkcs8(&key)
}

fn public_spki(pkcs8: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|_| data_error("invalid RSA private key"))?;
    encode_spki(&key.to_public_key())
}

/// Refuses a key whose modulus is below 1024 bits (D203): such keys can be
/// factored, so a signature or ciphertext made with one proves nothing.
/// Generation already refuses them; this holds every import to the same floor.
fn check_size(key: &impl PublicKeyParts) -> std::result::Result<(), OpError> {
    if (key.n().bits() as usize) < RsaPublicKey::MIN_SIZE {
        return Err(data_error(format!(
            "RSA keys under {} bits can be broken and are refused",
            RsaPublicKey::MIN_SIZE
        )));
    }
    Ok(())
}

fn import_pkcs8(der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(der).map_err(|_| data_error("invalid RSA private key"))?;
    check_size(&key)?;
    encode_pkcs8(&key)
}

fn import_spki(der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPublicKey::from_public_key_der(der).map_err(|_| data_error("invalid RSA public key"))?;
    check_size(&key)?;
    encode_spki(&key)
}

fn jwk_public_params(spki: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let key = RsaPublicKey::from_public_key_der(spki)
        .map_err(|_| data_error("invalid RSA public key"))?;
    Ok(frame(&[&trimmed(key.n()), &trimmed(key.e())]))
}

/// A JWK component: big-endian, without leading zero octets (RFC 7518
/// §2, `Base64urlUInt`). Variable-time in the value's length — export is the
/// one place a component's bytes are handed out on purpose.
fn trimmed(value: &BoxedUint) -> Vec<u8> {
    value.to_be_bytes_trimmed_vartime().into_vec()
}

fn jwk_private_params(pkcs8: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let mut key =
        RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|_| data_error("invalid RSA private key"))?;
    // Ensure the CRT params (dp/dq/qi) are available.
    key.precompute()
        .map_err(|_| operation_error("failed to precompute RSA CRT values"))?;
    let primes = key.primes();
    if primes.len() != 2 {
        return Err(not_supported("multi-prime RSA keys are not supported"));
    }
    let dp = trimmed(key.dp().ok_or_else(|| operation_error("missing RSA dp"))?);
    let dq = trimmed(key.dq().ok_or_else(|| operation_error("missing RSA dq"))?);
    let qi = trimmed(
        &key.crt_coefficient()
            .ok_or_else(|| operation_error("missing RSA qi"))?,
    );
    Ok(frame(&[
        &trimmed(key.n()),
        &trimmed(key.e()),
        &trimmed(key.d()),
        &trimmed(primes[0].as_ref()),
        &trimmed(primes[1].as_ref()),
        &dp,
        &dq,
        &qi,
    ]))
}

fn spki_from_jwk(n: &[u8], e: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    let key = RsaPublicKey::new(
        BoxedUint::from_be_slice_vartime(n),
        BoxedUint::from_be_slice_vartime(e),
    )
    .map_err(|_| data_error("invalid RSA public key components"))?;
    check_size(&key)?;
    encode_spki(&key)
}

fn pkcs8_from_jwk(
    n: &[u8],
    e: &[u8],
    d: &[u8],
    p: &[u8],
    q: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let invalid = || data_error("invalid RSA private key components");
    // `n` and `e` are public. The secrets are decoded at the modulus's width,
    // a public quantity, so their own lengths never choose the arithmetic.
    let n = BoxedUint::from_be_slice_vartime(n);
    let width = n.bits_precision();
    let secret = |bytes: &[u8]| BoxedUint::from_be_slice(bytes, width).map_err(|_| invalid());
    let key = RsaPrivateKey::from_components(
        n,
        BoxedUint::from_be_slice_vartime(e),
        secret(d)?,
        vec![secret(p)?, secret(q)?],
    )
    .map_err(|_| invalid())?;
    check_size(&key)?;
    encode_pkcs8(&key)
}

fn pkcs1v15_sign(
    entropy: &dyn Entropy,
    hash: &str,
    pkcs8: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|_| data_error("invalid RSA private key"))?;
    let hashed = prehash(hash, data)?;
    let mut rng = EntropyRng {
        entropy,
        failed: false,
    };
    let sig =
        with_oid_digest!(hash, D => key.sign_with_rng(&mut rng, Pkcs1v15Sign::new::<D>(), &hashed))
            .map_err(|_| operation_error("RSA signing failed"))?;
    if rng.failed {
        return Err(operation_error("entropy provider failed during signing"));
    }
    Ok(sig)
}

fn pkcs1v15_verify(
    hash: &str,
    spki: &[u8],
    signature: &[u8],
    data: &[u8],
) -> std::result::Result<bool, OpError> {
    let key = RsaPublicKey::from_public_key_der(spki)
        .map_err(|_| data_error("invalid RSA public key"))?;
    let hashed = prehash(hash, data)?;
    Ok(
        with_oid_digest!(hash, D => key.verify(Pkcs1v15Sign::new::<D>(), &hashed, signature))
            .is_ok(),
    )
}

fn pss_sign(
    entropy: &dyn Entropy,
    hash: &str,
    salt_len: usize,
    pkcs8: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|_| data_error("invalid RSA private key"))?;
    let hashed = prehash(hash, data)?;
    let mut rng = EntropyRng {
        entropy,
        failed: false,
    };
    let sig = with_oid_digest!(hash, D => key.sign_with_rng(&mut rng, Pss::<D>::new_with_salt(salt_len), &hashed))
        .map_err(|_| operation_error("RSA-PSS signing failed"))?;
    if rng.failed {
        return Err(operation_error("entropy provider failed during signing"));
    }
    Ok(sig)
}

fn pss_verify(
    hash: &str,
    salt_len: usize,
    spki: &[u8],
    signature: &[u8],
    data: &[u8],
) -> std::result::Result<bool, OpError> {
    let key = RsaPublicKey::from_public_key_der(spki)
        .map_err(|_| data_error("invalid RSA public key"))?;
    let hashed = prehash(hash, data)?;
    Ok(
        with_oid_digest!(hash, D => key.verify(Pss::<D>::new_with_salt(salt_len), &hashed, signature))
            .is_ok(),
    )
}

pub(crate) fn oaep_encrypt(
    entropy: &dyn Entropy,
    hash: &str,
    label: &[u8],
    spki: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let key = RsaPublicKey::from_public_key_der(spki)
        .map_err(|_| data_error("invalid RSA public key"))?;
    let mut rng = EntropyRng {
        entropy,
        failed: false,
    };
    let ct = with_oid_digest!(hash, D => key.encrypt(&mut rng, oaep_padding::<D>(label), data))
        .map_err(|_| operation_error("RSA-OAEP encryption failed"))?;
    if rng.failed {
        return Err(operation_error("entropy provider failed during encryption"));
    }
    Ok(ct)
}

fn oaep_decrypt(
    entropy: &dyn Entropy,
    hash: &str,
    label: &[u8],
    pkcs8: &[u8],
    data: &[u8],
) -> std::result::Result<Vec<u8>, OpError> {
    let key =
        RsaPrivateKey::from_pkcs8_der(pkcs8).map_err(|_| data_error("invalid RSA private key"))?;
    let mut rng = EntropyRng {
        entropy,
        failed: false,
    };
    // Blinded explicitly: without the crate's `getrandom` feature, plain
    // `decrypt` would run the private-key operation unblinded.
    let pt =
        with_oid_digest!(hash, D => key.decrypt_blinded(&mut rng, oaep_padding::<D>(label), data))
            .map_err(|_| operation_error("RSA-OAEP decryption failed"))?;
    if rng.failed {
        return Err(operation_error("entropy provider failed during decryption"));
    }
    Ok(pt)
}

/// An OAEP padding with `label` — any bytes, as WebCrypto allows.
fn oaep_padding<D>(label: &[u8]) -> Oaep<D>
where
    D: rsa::signature::digest::Digest + rsa::signature::digest::FixedOutputReset,
{
    if label.is_empty() {
        Oaep::<D>::new()
    } else {
        Oaep::<D>::new_with_label(label.to_vec())
    }
}

fn encode_pkcs8(key: &RsaPrivateKey) -> std::result::Result<Vec<u8>, OpError> {
    Ok(key
        .to_pkcs8_der()
        .map_err(|_| operation_error("failed to encode PKCS#8"))?
        .as_bytes()
        .to_vec())
}

fn encode_spki(key: &RsaPublicKey) -> std::result::Result<Vec<u8>, OpError> {
    Ok(key
        .to_public_key_der()
        .map_err(|_| operation_error("failed to encode SPKI"))?
        .as_bytes()
        .to_vec())
}
