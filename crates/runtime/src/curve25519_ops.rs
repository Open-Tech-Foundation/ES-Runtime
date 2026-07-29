//! Host ops for the WebCrypto Secure Curves algorithms: **Ed25519** signatures
//! and **X25519** key agreement (SPEC §2.10, DECISIONS D9).
//!
//! Both are "OKP" keys in JWK terms — a single 32-byte scalar, with no curve
//! parameter to choose — so one set of ops serves both, distinguished by an
//! algorithm name argument.
//!
//! No randomness is drawn here. A key is *made* from 32 bytes the prelude takes
//! from the [`Entropy`](es_runtime_providers::Entropy) provider, and Ed25519
//! signing is deterministic (RFC 8032 §5.1.6), so there is no nonce to source —
//! which is what keeps every byte of key material traceable to the injected
//! provider rather than an ambient OS RNG.

use es_runtime_engine::{Engine, OpDecl, OpError, Value};

use crate::Result;
use crate::crypto_ops::{arg_bytes, arg_str, data_error, not_supported, operation_error};

/// The DER object identifier body for each curve (RFC 8410 §3): the three bytes
/// following `06 03`. Ed25519 is 1.3.101.112, X25519 is 1.3.101.110.
const OID_ED25519: [u8; 3] = [0x2b, 0x65, 0x70];
const OID_X25519: [u8; 3] = [0x2b, 0x65, 0x6e];

/// Every key in this family is one 32-byte scalar or point.
const KEY_LEN: usize = 32;

/// Registers the Ed25519 / X25519 host ops.
pub(crate) fn install(engine: &mut dyn Engine) -> Result<()> {
    // The public key for a 32-byte secret. Ed25519 hashes the seed and clamps;
    // X25519 clamps and multiplies the basepoint — the curve decides, not us.
    engine.register_op(OpDecl::sync("okp_public", |args| {
        let curve = arg_str(&args, 0)?;
        let secret = seed(&arg_bytes(&args, 1)?)?;
        Ok(Value::Bytes(match Curve::parse(&curve)? {
            Curve::Ed25519 => ed25519_dalek::SigningKey::from_bytes(&secret)
                .verifying_key()
                .to_bytes()
                .to_vec(),
            Curve::X25519 => {
                x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(secret))
                    .to_bytes()
                    .to_vec()
            }
        }))
    }))?;

    engine.register_op(OpDecl::sync("ed25519_sign", |args| {
        let secret = seed(&arg_bytes(&args, 0)?)?;
        let message = arg_bytes(&args, 1)?;
        use ed25519_dalek::Signer as _;
        let signature = ed25519_dalek::SigningKey::from_bytes(&secret).sign(&message);
        Ok(Value::Bytes(signature.to_bytes().to_vec()))
    }))?;

    engine.register_op(OpDecl::sync("ed25519_verify", |args| {
        let public = arg_bytes(&args, 0)?;
        let signature = arg_bytes(&args, 1)?;
        let message = arg_bytes(&args, 2)?;
        Ok(Value::Bool(ed25519_verify(&public, &signature, &message)))
    }))?;

    engine.register_op(OpDecl::sync("x25519_derive", |args| {
        let secret = seed(&arg_bytes(&args, 0)?)?;
        let peer = seed(&arg_bytes(&args, 1)?)?;
        let shared = x25519_dalek::StaticSecret::from(secret)
            .diffie_hellman(&x25519_dalek::PublicKey::from(peer));
        // A low-order peer point drives the shared secret to all zeros. The
        // spec requires that to be an error rather than a usable secret, and
        // the crate reports it without branching on the secret itself.
        if !shared.was_contributory() {
            return Err(operation_error(
                "X25519 produced an all-zero shared secret (the peer key is low-order)",
            ));
        }
        Ok(Value::Bytes(shared.to_bytes().to_vec()))
    }))?;

    // --- DER key formats (RFC 8410) ---
    engine.register_op(OpDecl::sync("okp_export_pkcs8", |args| {
        let curve = Curve::parse(&arg_str(&args, 0)?)?;
        let secret = seed(&arg_bytes(&args, 1)?)?;
        Ok(Value::Bytes(export_pkcs8(curve, &secret)))
    }))?;

    engine.register_op(OpDecl::sync("okp_import_pkcs8", |args| {
        let curve = Curve::parse(&arg_str(&args, 0)?)?;
        let der = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(import_pkcs8(curve, &der)?))
    }))?;

    engine.register_op(OpDecl::sync("okp_export_spki", |args| {
        let curve = Curve::parse(&arg_str(&args, 0)?)?;
        let public = seed(&arg_bytes(&args, 1)?)?;
        Ok(Value::Bytes(export_spki(curve, &public)))
    }))?;

    engine.register_op(OpDecl::sync("okp_import_spki", |args| {
        let curve = Curve::parse(&arg_str(&args, 0)?)?;
        let der = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(import_spki(curve, &der)?))
    }))?;

    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Curve {
    Ed25519,
    X25519,
}

impl Curve {
    fn parse(name: &str) -> std::result::Result<Self, OpError> {
        match name {
            "Ed25519" => Ok(Curve::Ed25519),
            "X25519" => Ok(Curve::X25519),
            other => Err(not_supported(format!("unsupported curve: {other}"))),
        }
    }

    fn oid(self) -> [u8; 3] {
        match self {
            Curve::Ed25519 => OID_ED25519,
            Curve::X25519 => OID_X25519,
        }
    }
}

/// Narrows a byte slice to the fixed 32-byte scalar every key here is.
fn seed(bytes: &[u8]) -> std::result::Result<[u8; KEY_LEN], OpError> {
    bytes
        .try_into()
        .map_err(|_| data_error("a Curve25519 key must be 32 bytes"))
}

/// Verifies an Ed25519 signature. Every failure — malformed key, malformed
/// signature, wrong signature — is the same `false`: `subtle.verify` has no
/// other answer to give.
fn ed25519_verify(public: &[u8], signature: &[u8], message: &[u8]) -> bool {
    use ed25519_dalek::Verifier as _;
    let (Ok(public), Ok(signature)) = (seed(public), <[u8; 64]>::try_from(signature)) else {
        return false;
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(&public) else {
        return false;
    };
    key.verify(message, &ed25519_dalek::Signature::from_bytes(&signature))
        .is_ok()
}

// ---- RFC 8410 DER ----------------------------------------------------------
//
// These two structures have exactly one shape for a 32-byte key, so they are
// built and matched as fixed byte layouts rather than parsed by a general DER
// reader. Being strict is the point: anything that is not this encoding is not
// something we should be reading key material out of.
//
//   PrivateKeyInfo   30 2e 02 01 00 30 05 06 03 <oid> 04 22 04 20 <32 bytes>
//   SubjectPublicKeyInfo  30 2a 30 05 06 03 <oid> 03 21 00 <32 bytes>
//
// A PKCS#8 v2 (`OneAsymmetricKey`) body carries the public key in a trailing
// `[1]` field and a version of 1; the private key sits in the same place, so
// imports accept it and ignore the tail.

const PKCS8_LEN: usize = 48;
const SPKI_LEN: usize = 44;

fn export_pkcs8(curve: Curve, secret: &[u8; KEY_LEN]) -> Vec<u8> {
    let mut der = Vec::with_capacity(PKCS8_LEN);
    der.extend_from_slice(&[0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03]);
    der.extend_from_slice(&curve.oid());
    der.extend_from_slice(&[0x04, 0x22, 0x04, 0x20]);
    der.extend_from_slice(secret);
    der
}

fn import_pkcs8(curve: Curve, der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    // Everything up to the version byte, which is 0 for v1 and 1 for v2.
    if der.len() < PKCS8_LEN || der[0] != 0x30 || der[2..4] != [0x02, 0x01] || der[4] > 1 {
        return Err(data_error("not a PKCS#8 private key"));
    }
    if der[5..9] != [0x30, 0x05, 0x06, 0x03] || der[9..12] != curve.oid() {
        return Err(data_error("PKCS#8 algorithm does not match the key type"));
    }
    if der[12..16] != [0x04, 0x22, 0x04, 0x20] {
        return Err(data_error("unexpected PKCS#8 private-key encoding"));
    }
    Ok(der[16..PKCS8_LEN].to_vec())
}

fn export_spki(curve: Curve, public: &[u8; KEY_LEN]) -> Vec<u8> {
    let mut der = Vec::with_capacity(SPKI_LEN);
    der.extend_from_slice(&[0x30, 0x2a, 0x30, 0x05, 0x06, 0x03]);
    der.extend_from_slice(&curve.oid());
    der.extend_from_slice(&[0x03, 0x21, 0x00]);
    der.extend_from_slice(public);
    der
}

fn import_spki(curve: Curve, der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    if der.len() != SPKI_LEN || der[0..6] != [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03] {
        return Err(data_error("not a SubjectPublicKeyInfo"));
    }
    if der[6..9] != curve.oid() {
        return Err(data_error("SPKI algorithm does not match the key type"));
    }
    if der[9..12] != [0x03, 0x21, 0x00] {
        return Err(data_error("unexpected SPKI public-key encoding"));
    }
    Ok(der[12..SPKI_LEN].to_vec())
}

/// Fuzz entry: the hand-written RFC 8410 DER parsers (see [`crate::fuzz`]).
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_import(curve: &str, der: &[u8]) {
    if let Ok(curve) = Curve::parse(curve) {
        let _ = import_pkcs8(curve, der);
        let _ = import_spki(curve, der);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 8032 §7.1 TEST 2: a one-byte message under a published key pair.
    #[test]
    fn ed25519_matches_rfc8032_test_2() {
        let secret = hex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb");
        let public = ed25519_dalek::SigningKey::from_bytes(&seed(&secret).unwrap())
            .verifying_key()
            .to_bytes();
        assert_eq!(
            public.to_vec(),
            hex("3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c")
        );

        use ed25519_dalek::Signer as _;
        let signature =
            ed25519_dalek::SigningKey::from_bytes(&seed(&secret).unwrap()).sign(&[0x72]);
        assert_eq!(
            signature.to_bytes().to_vec(),
            hex(
                "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
                 085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"
            )
        );
        assert!(ed25519_verify(&public, &signature.to_bytes(), &[0x72]));
        // A different message under the same signature must not verify.
        assert!(!ed25519_verify(&public, &signature.to_bytes(), &[0x73]));
    }

    /// RFC 7748 §6.1: Alice and Bob agree on the same secret.
    #[test]
    fn x25519_matches_rfc7748_section_6_1() {
        let alice = seed(&hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ))
        .unwrap();
        let bob = seed(&hex(
            "5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb",
        ))
        .unwrap();
        let alice_pub = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(alice));
        let bob_pub = x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(bob));
        assert_eq!(
            alice_pub.to_bytes().to_vec(),
            hex("8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a")
        );

        let shared = x25519_dalek::StaticSecret::from(alice).diffie_hellman(&bob_pub);
        assert_eq!(
            shared.to_bytes().to_vec(),
            hex("4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742")
        );
        assert_eq!(
            x25519_dalek::StaticSecret::from(bob)
                .diffie_hellman(&alice_pub)
                .to_bytes(),
            shared.to_bytes()
        );
    }

    /// The DER encodings are fixed layouts, so a round trip plus the exact
    /// prefix is the whole contract.
    #[test]
    fn der_round_trips_and_pins_the_algorithm() {
        let key = [7u8; KEY_LEN];
        for (curve, oid) in [(Curve::Ed25519, 0x70), (Curve::X25519, 0x6e)] {
            let pkcs8 = export_pkcs8(curve, &key);
            assert_eq!(pkcs8.len(), PKCS8_LEN);
            assert_eq!(pkcs8[11], oid);
            assert_eq!(import_pkcs8(curve, &pkcs8).unwrap(), key.to_vec());

            let spki = export_spki(curve, &key);
            assert_eq!(spki.len(), SPKI_LEN);
            assert_eq!(spki[8], oid);
            assert_eq!(import_spki(curve, &spki).unwrap(), key.to_vec());
        }

        // A key exported for one curve must not import as the other: the OID is
        // the only thing distinguishing them, so this is the check that matters.
        let ed = export_pkcs8(Curve::Ed25519, &key);
        assert!(import_pkcs8(Curve::X25519, &ed).is_err());
        let ed_pub = export_spki(Curve::Ed25519, &key);
        assert!(import_spki(Curve::X25519, &ed_pub).is_err());

        assert!(import_pkcs8(Curve::Ed25519, &[0x30, 0x2e]).is_err());
        assert!(import_spki(Curve::Ed25519, &[]).is_err());
    }

    /// A PKCS#8 v2 body carries the public key after the private one; the
    /// private key is in the same place, so it must still import.
    #[test]
    fn pkcs8_v2_with_a_trailing_public_key_imports() {
        let key = [3u8; KEY_LEN];
        let mut v2 = export_pkcs8(Curve::Ed25519, &key);
        v2[4] = 0x01; // version 1 == OneAsymmetricKey (v2)
        v2.extend_from_slice(&[0x81, 0x21, 0x00]);
        v2.extend_from_slice(&[9u8; KEY_LEN]);
        assert_eq!(import_pkcs8(Curve::Ed25519, &v2).unwrap(), key.to_vec());
    }

    fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
}
