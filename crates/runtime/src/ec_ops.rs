//! Host ops backing elliptic-curve WebCrypto: ECDSA (sign/verify) and ECDH
//! (key agreement) over the NIST P-256/P-384/P-521 curves (SPEC §2.10), using
//! the RustCrypto curve crates (DECISIONS.md D9).
//!
//! Keys cross the op boundary in standard serializations: **private** keys as
//! PKCS#8 DER, **public** keys as SEC1 uncompressed points (`0x04 || X || Y`).
//! The prelude assembles JWK from the raw coordinates/scalar these ops expose,
//! and converts SPKI via [`ec_export_spki`]/[`ec_import_spki`].
//!
//! ECDSA honours an arbitrary `algorithm.hash`: we compute the message digest
//! here with `sha2` 0.11 and feed the prehash to the curve's `sign_prehash`
//! (the curve crates' built-in `DigestSigner` is fixed to a single hash). The
//! curve crates still sit on the older `digest` 0.10 generation; see the note
//! in the workspace `Cargo.toml`.

use std::sync::Arc;

use es_runtime_engine::{Engine, OpDecl, OpError, Value};
use es_runtime_providers::Entropy;

use p256::elliptic_curve::rand_core::{CryptoRng, RngCore};

use crate::Result;
use crate::crypto_ops::{arg_bytes, arg_str, data_error, not_supported, operation_error};

/// Adapts an [`Entropy`] provider to the `rand_core` 0.6 traits the curve and
/// RSA crates' key generation expects. `fill_bytes` cannot signal failure, so
/// a provider error is latched in `failed` and checked after generation; the
/// produced key is discarded in that case. Shared with [`crate::rsa_ops`].
pub(crate) struct EntropyRng<'a> {
    pub(crate) entropy: &'a dyn Entropy,
    pub(crate) failed: bool,
}

impl RngCore for EntropyRng<'_> {
    fn next_u32(&mut self) -> u32 {
        let mut b = [0u8; 4];
        self.fill_bytes(&mut b);
        u32::from_le_bytes(b)
    }

    fn next_u64(&mut self) -> u64 {
        let mut b = [0u8; 8];
        self.fill_bytes(&mut b);
        u64::from_le_bytes(b)
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        if self.entropy.fill(dest).is_err() {
            self.failed = true;
            dest.fill(0);
        }
    }

    fn try_fill_bytes(
        &mut self,
        dest: &mut [u8],
    ) -> std::result::Result<(), p256::elliptic_curve::rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}

impl CryptoRng for EntropyRng<'_> {}

/// Computes the message prehash for ECDSA/RSA, honouring `algorithm.hash` (so
/// e.g. ECDSA over P-256 with SHA-512 works). Uses our `sha2` 0.11. Shared
/// with [`crate::rsa_ops`].
pub(crate) fn prehash(hash: &str, data: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
    use sha2::Digest as _;
    Ok(match hash {
        "SHA-1" => sha1::Sha1::digest(data).to_vec(),
        "SHA-256" => sha2::Sha256::digest(data).to_vec(),
        "SHA-384" => sha2::Sha384::digest(data).to_vec(),
        "SHA-512" => sha2::Sha512::digest(data).to_vec(),
        other => return Err(not_supported(format!("unsupported ECDSA hash: {other}"))),
    })
}

/// Generates the per-curve operations. Each curve crate (`p256`/`p384`/`p521`)
/// exposes the same API surface, so the bodies are identical up to the curve
/// type; a macro keeps them in lock-step without leaking the generic
/// trait-bound machinery into the dispatch layer.
macro_rules! curve_impl {
    ($module:ident, $crate_path:ident) => {
        mod $module {
            use super::{EntropyRng, OpError, data_error, operation_error};
            use es_runtime_providers::Entropy;

            use curve::ecdsa::signature::hazmat::{PrehashVerifier, RandomizedPrehashSigner};
            use curve::ecdsa::{Signature, SigningKey, VerifyingKey};
            use curve::elliptic_curve::pkcs8::spki::{DecodePublicKey, EncodePublicKey};
            use curve::elliptic_curve::pkcs8::{DecodePrivateKey, EncodePrivateKey};
            use curve::elliptic_curve::sec1::ToEncodedPoint;
            use curve::{PublicKey, SecretKey};
            use $crate_path as curve;

            /// Generates a fresh private key, returned as PKCS#8 DER.
            pub fn generate(entropy: &dyn Entropy) -> std::result::Result<Vec<u8>, OpError> {
                let mut rng = EntropyRng {
                    entropy,
                    failed: false,
                };
                let sk = SecretKey::random(&mut rng);
                if rng.failed {
                    return Err(operation_error(
                        "entropy provider failed during key generation",
                    ));
                }
                Ok(sk
                    .to_pkcs8_der()
                    .map_err(|_| operation_error("failed to encode PKCS#8"))?
                    .as_bytes()
                    .to_vec())
            }

            /// SEC1 uncompressed public point for a PKCS#8 private key.
            pub fn public_point(pkcs8: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let sk = SecretKey::from_pkcs8_der(pkcs8)
                    .map_err(|_| data_error("invalid PKCS#8 private key"))?;
                Ok(sk.public_key().to_encoded_point(false).as_bytes().to_vec())
            }

            /// Raw private scalar (field-width, big-endian) for JWK `d`.
            pub fn private_scalar(pkcs8: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let sk = SecretKey::from_pkcs8_der(pkcs8)
                    .map_err(|_| data_error("invalid PKCS#8 private key"))?;
                Ok(sk.to_bytes().to_vec())
            }

            /// Validates and re-encodes a PKCS#8 private key.
            pub fn import_pkcs8(der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let sk =
                    SecretKey::from_pkcs8_der(der).map_err(|_| data_error("invalid PKCS#8 key"))?;
                Ok(sk
                    .to_pkcs8_der()
                    .map_err(|_| operation_error("failed to encode PKCS#8"))?
                    .as_bytes()
                    .to_vec())
            }

            /// Builds a PKCS#8 private key from a raw scalar (JWK `d`).
            pub fn pkcs8_from_scalar(d: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let sk =
                    SecretKey::from_slice(d).map_err(|_| data_error("invalid private scalar"))?;
                Ok(sk
                    .to_pkcs8_der()
                    .map_err(|_| operation_error("failed to encode PKCS#8"))?
                    .as_bytes()
                    .to_vec())
            }

            /// SPKI DER → SEC1 uncompressed public point.
            pub fn import_spki(der: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let pk = PublicKey::from_public_key_der(der)
                    .map_err(|_| data_error("invalid SPKI public key"))?;
                Ok(pk.to_encoded_point(false).as_bytes().to_vec())
            }

            /// SEC1 uncompressed public point → SPKI DER.
            pub fn export_spki(sec1: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let pk = PublicKey::from_sec1_bytes(sec1)
                    .map_err(|_| data_error("invalid public point"))?;
                Ok(pk
                    .to_public_key_der()
                    .map_err(|_| operation_error("failed to encode SPKI"))?
                    .as_bytes()
                    .to_vec())
            }

            /// ECDSA sign over a prehashed message → fixed-width `r || s`.
            ///
            /// Signs with the nonce randomness drawn from the injected
            /// [`Entropy`] provider (hedged signing) rather than ambient
            /// `OsRng` — P-521's deterministic path would otherwise reach for
            /// `OsRng` directly, breaking the no-ambient-authority property.
            pub fn sign(
                entropy: &dyn Entropy,
                prehash: &[u8],
                pkcs8: &[u8],
            ) -> std::result::Result<Vec<u8>, OpError> {
                let sk = SecretKey::from_pkcs8_der(pkcs8)
                    .map_err(|_| data_error("invalid PKCS#8 private key"))?;
                let signing = SigningKey::from_slice(&sk.to_bytes())
                    .map_err(|_| operation_error("invalid signing key"))?;
                let mut rng = EntropyRng {
                    entropy,
                    failed: false,
                };
                let sig: Signature = signing
                    .sign_prehash_with_rng(&mut rng, prehash)
                    .map_err(|_| operation_error("ECDSA signing failed"))?;
                if rng.failed {
                    return Err(operation_error("entropy provider failed during signing"));
                }
                Ok(sig.to_bytes().to_vec())
            }

            /// ECDSA verify. Malformed signatures verify as `false` (per spec),
            /// not an error.
            pub fn verify(
                prehash: &[u8],
                sec1: &[u8],
                signature: &[u8],
            ) -> std::result::Result<bool, OpError> {
                let vk = VerifyingKey::from_sec1_bytes(sec1)
                    .map_err(|_| data_error("invalid public point"))?;
                let Ok(sig) = Signature::from_slice(signature) else {
                    return Ok(false);
                };
                Ok(vk.verify_prehash(prehash, &sig).is_ok())
            }

            /// ECDH: raw shared secret (the agreed X coordinate, field-width).
            pub fn ecdh(pkcs8: &[u8], peer_sec1: &[u8]) -> std::result::Result<Vec<u8>, OpError> {
                let sk = SecretKey::from_pkcs8_der(pkcs8)
                    .map_err(|_| data_error("invalid PKCS#8 private key"))?;
                let pk = PublicKey::from_sec1_bytes(peer_sec1)
                    .map_err(|_| data_error("invalid peer public point"))?;
                let shared = curve::ecdh::diffie_hellman(sk.to_nonzero_scalar(), pk.as_affine());
                Ok(shared.raw_secret_bytes().to_vec())
            }
        }
    };
}

curve_impl!(p256_curve, p256);
curve_impl!(p384_curve, p384);
curve_impl!(p521_curve, p521);

/// Dispatches a curve-keyed call to the matching per-curve module.
macro_rules! by_curve {
    ($curve:expr, $func:ident ( $($arg:expr),* )) => {
        match $curve {
            "P-256" => p256_curve::$func($($arg),*),
            "P-384" => p384_curve::$func($($arg),*),
            "P-521" => p521_curve::$func($($arg),*),
            other => return Err(not_supported(format!("unsupported curve: {other}"))),
        }
    };
}

/// Registers the elliptic-curve host ops.
pub(crate) fn install(engine: &mut dyn Engine, entropy: Arc<dyn Entropy>) -> Result<()> {
    let gen_entropy = entropy.clone();
    engine.register_op(OpDecl::sync("ec_generate_pkcs8", move |args| {
        let curve = arg_str(&args, 0)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            generate(gen_entropy.as_ref())
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ec_public_point", |args| {
        let curve = arg_str(&args, 0)?;
        let pkcs8 = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            public_point(&pkcs8)
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ec_private_scalar", |args| {
        let curve = arg_str(&args, 0)?;
        let pkcs8 = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            private_scalar(&pkcs8)
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ec_import_pkcs8", |args| {
        let curve = arg_str(&args, 0)?;
        let der = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(curve.as_str(), import_pkcs8(&der))?))
    }))?;

    engine.register_op(OpDecl::sync("ec_pkcs8_from_scalar", |args| {
        let curve = arg_str(&args, 0)?;
        let d = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            pkcs8_from_scalar(&d)
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ec_import_spki", |args| {
        let curve = arg_str(&args, 0)?;
        let der = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(curve.as_str(), import_spki(&der))?))
    }))?;

    engine.register_op(OpDecl::sync("ec_export_spki", |args| {
        let curve = arg_str(&args, 0)?;
        let sec1 = arg_bytes(&args, 1)?;
        Ok(Value::Bytes(by_curve!(curve.as_str(), export_spki(&sec1))?))
    }))?;

    engine.register_op(OpDecl::sync("ecdsa_sign", move |args| {
        let curve = arg_str(&args, 0)?;
        let hash = arg_str(&args, 1)?;
        let pkcs8 = arg_bytes(&args, 2)?;
        let data = arg_bytes(&args, 3)?;
        let digest = prehash(&hash, &data)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            sign(entropy.as_ref(), &digest, &pkcs8)
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ecdsa_verify", |args| {
        let curve = arg_str(&args, 0)?;
        let hash = arg_str(&args, 1)?;
        let sec1 = arg_bytes(&args, 2)?;
        let signature = arg_bytes(&args, 3)?;
        let data = arg_bytes(&args, 4)?;
        let digest = prehash(&hash, &data)?;
        Ok(Value::Bool(by_curve!(
            curve.as_str(),
            verify(&digest, &sec1, &signature)
        )?))
    }))?;

    engine.register_op(OpDecl::sync("ecdh_derive", |args| {
        let curve = arg_str(&args, 0)?;
        let pkcs8 = arg_bytes(&args, 1)?;
        let peer = arg_bytes(&args, 2)?;
        Ok(Value::Bytes(by_curve!(
            curve.as_str(),
            ecdh(&pkcs8, &peer)
        )?))
    }))?;

    Ok(())
}
