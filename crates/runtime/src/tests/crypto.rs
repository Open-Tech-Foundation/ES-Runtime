//! Randomness and `crypto.subtle`.

use super::*;

#[test]
fn get_random_values_fills_in_place() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const a = new Uint8Array(16); const r = crypto.getRandomValues(a); \
         return r === a && a.some((x) => x !== 0); })()",
    );
}

#[test]
fn random_uuid_is_well_formed_v4() {
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID())",
    );
}

#[test]
fn random_uuids_stay_well_formed_and_distinct_across_pool_refills() {
    // 300 UUIDs cross two refills of the 128-UUID entropy pool.
    let _g = v8_guard();
    let mut rt = runtime();
    assert_true(
        &mut rt,
        "(() => { const re = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/; \
         const seen = new Set(); \
         for (let i = 0; i < 300; i++) { const u = crypto.randomUUID(); if (!re.test(u)) return false; seen.add(u); } \
         return seen.size === 300; })()",
    );
}

#[test]
fn a_restored_runtime_draws_uuids_from_its_own_entropy() {
    // The pool must never be baked into the snapshot: every process restores
    // the same snapshot, and would otherwise hand out the same UUIDs. Built
    // with one entropy source and restored with another, the first UUID is
    // made of the restoring runtime's bytes.
    struct Constant;
    impl Entropy for Constant {
        fn fill(
            &self,
            dest: &mut [u8],
        ) -> std::result::Result<(), es_runtime_providers::ProviderError> {
            dest.fill(0xaa);
            Ok(())
        }
    }
    let _g = v8_guard();
    let blob = Runtime::build_snapshot(&test_providers()).expect("build snapshot");
    let providers = HostProviders::new(
        Arc::new(FixedClock {
            monotonic: 0,
            wall: 0,
        }),
        Arc::new(TestConsole::default()),
        Arc::new(MockNet::stub()),
        Arc::new(Constant),
    )
    .with_ports(Arc::new(TestPortHub::default()));
    let mut rt = Runtime::with_snapshot(blob, providers).expect("restore");
    assert_eq!(
        rt.eval("crypto.randomUUID()").unwrap(),
        Value::String("aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa".into())
    );
}

#[test]
fn subtle_digest_matches_known_sha256_vector() {
    let _g = v8_guard();
    let mut rt = runtime();
    // SHA-256("abc").
    let out = eval_async(
        &mut rt,
        "const h = await crypto.subtle.digest('SHA-256', new TextEncoder().encode('abc')); \
         return [...new Uint8Array(h)].map((b) => b.toString(16).padStart(2, '0')).join('');",
    );
    assert_eq!(
        out,
        Value::String("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into())
    );
}

#[test]
fn subtle_hmac_signs_and_verifies() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const key = await crypto.subtle.importKey('raw', enc.encode('secret'), \
           { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']); \
         const sig = await crypto.subtle.sign('HMAC', key, enc.encode('message')); \
         const good = await crypto.subtle.verify('HMAC', key, sig, enc.encode('message')); \
         const bad = await crypto.subtle.verify('HMAC', key, sig, enc.encode('tampered')); \
         return good === true && bad === false;",
    );
    assert_eq!(out, Value::Bool(true));
}

#[test]
fn subtle_aes_gcm_round_trips() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, true, ['encrypt', 'decrypt']); \
         const iv = crypto.getRandomValues(new Uint8Array(12)); \
         const pt = new TextEncoder().encode('hello gcm'); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, pt); \
         const out = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); \
         return new TextDecoder().decode(out);",
    );
    assert_eq!(out, Value::String("hello gcm".into()));
}

#[test]
fn subtle_aes_gcm_rejects_tampered_ciphertext() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt', 'decrypt']); \
         const iv = crypto.getRandomValues(new Uint8Array(12)); \
         const ct = new Uint8Array(await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode('data'))); \
         ct[0] ^= 0xff; \
         await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); return 'no-error';",
    );
    match out {
        Value::String(s) => assert!(
            s.starts_with("ERR:") && (s.contains("OperationError") || s.contains("decryption")),
            "expected OperationError, got {s}"
        ),
        other => panic!("expected rejection, got {other:?}"),
    }
}

#[test]
fn subtle_aes_cbc_round_trips() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const key = await crypto.subtle.generateKey({ name: 'AES-CBC', length: 256 }, true, ['encrypt', 'decrypt']); \
         const iv = crypto.getRandomValues(new Uint8Array(16)); \
         const pt = new TextEncoder().encode('hello cbc, longer than one block'); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-CBC', iv }, key, pt); \
         const out = await crypto.subtle.decrypt({ name: 'AES-CBC', iv }, key, ct); \
         return new TextDecoder().decode(out);",
    );
    assert_eq!(
        out,
        Value::String("hello cbc, longer than one block".into())
    );
}

#[test]
fn subtle_aes_cbc_known_answer() {
    // FIPS-197 / NIST SP 800-38A AES-128-CBC, first block of F.2.1.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
         const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
         const key = await crypto.subtle.importKey('raw', hex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CBC', false, ['encrypt']); \
         const iv = hex('000102030405060708090a0b0c0d0e0f'); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-CBC', iv }, key, hex('6bc1bee22e409f96e93d7e117393172a')); \
         return toHex(ct).slice(0, 32);",
    );
    // Expected first ciphertext block for that vector.
    assert_eq!(
        out,
        Value::String("7649abac8119b246cee98e9b12e9197d".into())
    );
}

/// RFC 3394 §4.1: wrap 128 bits of key data with a 128-bit KEK. A published
/// vector is the only way to know the wrap is AES-KW and not merely
/// something that round-trips with itself.
#[test]
fn subtle_aes_kw_rfc3394_vector() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
         const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
         const kek = await crypto.subtle.importKey('raw', hex('000102030405060708090a0b0c0d0e0f'), 'AES-KW', false, ['wrapKey', 'unwrapKey']); \
         const target = await crypto.subtle.importKey('raw', hex('00112233445566778899aabbccddeeff'), { name: 'AES-CBC' }, true, ['encrypt']); \
         const wrapped = await crypto.subtle.wrapKey('raw', target, kek, 'AES-KW'); \
         const back = await crypto.subtle.unwrapKey('raw', wrapped, kek, 'AES-KW', { name: 'AES-CBC' }, true, ['encrypt']); \
         const raw = await crypto.subtle.exportKey('raw', back); \
         return toHex(wrapped) + '|' + toHex(raw);",
    );
    assert_eq!(
        out,
        Value::String(
            "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5\
             |00112233445566778899aabbccddeeff"
                .into()
        )
    );
}

/// The integrity check is the point of AES-KW: a flipped bit must fail the
/// unwrap rather than hand back wrong key material.
#[test]
fn subtle_aes_kw_rejects_tampered_ciphertext() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const kek = await crypto.subtle.generateKey({ name: 'AES-KW', length: 256 }, true, ['wrapKey', 'unwrapKey']); \
         const target = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt']); \
         const wrapped = new Uint8Array(await crypto.subtle.wrapKey('raw', target, kek, 'AES-KW')); \
         wrapped[3] ^= 1; \
         try { \
           await crypto.subtle.unwrapKey('raw', wrapped, kek, 'AES-KW', { name: 'AES-GCM' }, true, ['encrypt']); \
           return 'accepted'; \
         } catch (e) { return e.name; }",
    );
    assert_eq!(out, Value::String("OperationError".into()));
}

#[test]
fn subtle_aes_ctr_round_trips() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const key = await crypto.subtle.generateKey({ name: 'AES-CTR', length: 128 }, true, ['encrypt', 'decrypt']); \
         const counter = crypto.getRandomValues(new Uint8Array(16)); \
         const pt = new TextEncoder().encode('hello ctr'); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-CTR', counter, length: 64 }, key, pt); \
         const out = await crypto.subtle.decrypt({ name: 'AES-CTR', counter, length: 64 }, key, ct); \
         return new TextDecoder().decode(out);",
    );
    assert_eq!(out, Value::String("hello ctr".into()));
}

#[test]
fn subtle_aes_ctr_known_answer() {
    // NIST SP 800-38A F.5.1 CTR-AES128.Encrypt, first block (128-bit counter).
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
         const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
         const key = await crypto.subtle.importKey('raw', hex('2b7e151628aed2a6abf7158809cf4f3c'), 'AES-CTR', false, ['encrypt']); \
         const counter = hex('f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff'); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-CTR', counter, length: 128 }, key, hex('6bc1bee22e409f96e93d7e117393172a')); \
         return toHex(ct);",
    );
    assert_eq!(
        out,
        Value::String("874d6191b620e3261bef6864990db6ce".into())
    );
}

#[test]
fn subtle_hkdf_rfc5869_test_case_1() {
    // RFC 5869 Appendix A.1 (SHA-256).
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const hex = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16))); \
         const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
         const ikm = new Uint8Array(22).fill(0x0b); \
         const key = await crypto.subtle.importKey('raw', ikm, 'HKDF', false, ['deriveBits']); \
         const bits = await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: hex('000102030405060708090a0b0c'), info: hex('f0f1f2f3f4f5f6f7f8f9') }, key, 42 * 8); \
         return toHex(bits);",
    );
    assert_eq!(
        out,
        Value::String(
            "3cb25f25faacd57a90434f64d0362f2a2d2d0a90cf1a5a4c5db02d56ecc4c5bf34007208d5b887185865"
                .into()
        )
    );
}

#[test]
fn subtle_pbkdf2_rfc6070_vector() {
    // RFC 6070 PBKDF2-HMAC-SHA1, P="password" S="salt" c=1 dkLen=20.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const toHex = (a) => [...new Uint8Array(a)].map((b) => b.toString(16).padStart(2, '0')).join(''); \
         const enc = new TextEncoder(); \
         const key = await crypto.subtle.importKey('raw', enc.encode('password'), 'PBKDF2', false, ['deriveBits']); \
         const bits = await crypto.subtle.deriveBits({ name: 'PBKDF2', hash: 'SHA-1', salt: enc.encode('salt'), iterations: 1 }, key, 20 * 8); \
         return toHex(bits);",
    );
    assert_eq!(
        out,
        Value::String("0c60c80f961f0e71f3a9b524af6012062fe037a6".into())
    );
}

#[test]
fn subtle_derive_key_then_aes_gcm_round_trips() {
    // deriveKey: PBKDF2 → AES-GCM key, used end-to-end.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const base = await crypto.subtle.importKey('raw', enc.encode('correct horse'), 'PBKDF2', false, ['deriveKey']); \
         const key = await crypto.subtle.deriveKey({ name: 'PBKDF2', hash: 'SHA-256', salt: enc.encode('battery'), iterations: 200 }, base, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']); \
         const iv = crypto.getRandomValues(new Uint8Array(12)); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, enc.encode('staple')); \
         const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, key, ct); \
         return new TextDecoder().decode(pt);",
    );
    assert_eq!(out, Value::String("staple".into()));
}

#[test]
fn subtle_ecdsa_p256_sign_verify_round_trips() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']); \
         const data = enc.encode('sign me'); \
         const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, kp.privateKey, data); \
         const good = await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, kp.publicKey, sig, data); \
         const bad = await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, kp.publicKey, sig, enc.encode('tampered')); \
         return `${good}:${bad}`;",
    );
    assert_eq!(out, Value::String("true:false".into()));
}

#[test]
fn subtle_ecdsa_p521_sha512_round_trips() {
    // Exercises the divergent P-521 signing path (entropy-routed nonce).
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const kp = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-521' }, true, ['sign', 'verify']); \
         const data = enc.encode('p521'); \
         const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-512' }, kp.privateKey, data); \
         return String(await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-512' }, kp.publicKey, sig, data));",
    );
    assert_eq!(out, Value::String("true".into()));
}

#[test]
fn subtle_ec_key_export_import_all_formats_round_trip() {
    // Export the keys to every format, re-import, and confirm a signature
    // from the original private key verifies under each re-imported public.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); const data = enc.encode('formats'); \
         const algo = { name: 'ECDSA', namedCurve: 'P-384' }; \
         const kp = await crypto.subtle.generateKey(algo, true, ['sign', 'verify']); \
         const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, kp.privateKey, data); \
         const pkcs8 = await crypto.subtle.exportKey('pkcs8', kp.privateKey); \
         const spki = await crypto.subtle.exportKey('spki', kp.publicKey); \
         const raw = await crypto.subtle.exportKey('raw', kp.publicKey); \
         const jwkPub = await crypto.subtle.exportKey('jwk', kp.publicKey); \
         const jwkPriv = await crypto.subtle.exportKey('jwk', kp.privateKey); \
         const priv2 = await crypto.subtle.importKey('pkcs8', pkcs8, algo, true, ['sign']); \
         const sig2 = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, priv2, data); \
         const fromSpki = await crypto.subtle.importKey('spki', spki, algo, true, ['verify']); \
         const fromRaw = await crypto.subtle.importKey('raw', raw, algo, true, ['verify']); \
         const fromJwk = await crypto.subtle.importKey('jwk', jwkPub, algo, true, ['verify']); \
         const fromJwkPriv = await crypto.subtle.importKey('jwk', jwkPriv, algo, true, ['sign']); \
         const sig3 = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-384' }, fromJwkPriv, data); \
         const v = (k, s) => crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-384' }, k, s, data); \
         const results = [await v(fromSpki, sig), await v(fromRaw, sig), await v(fromJwk, sig2), await v(fromSpki, sig3)]; \
         return results.every((r) => r === true) ? 'all-ok' : 'mismatch';",
    );
    assert_eq!(out, Value::String("all-ok".into()));
}

#[test]
fn subtle_ecdh_agreement_is_symmetric() {
    // Both parties derive the same shared secret (P-256).
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const algo = { name: 'ECDH', namedCurve: 'P-256' }; \
         const a = await crypto.subtle.generateKey(algo, true, ['deriveBits']); \
         const b = await crypto.subtle.generateKey(algo, true, ['deriveBits']); \
         const toHex = (buf) => [...new Uint8Array(buf)].map((x) => x.toString(16).padStart(2, '0')).join(''); \
         const ab = await crypto.subtle.deriveBits({ name: 'ECDH', public: b.publicKey }, a.privateKey, 256); \
         const ba = await crypto.subtle.deriveBits({ name: 'ECDH', public: a.publicKey }, b.privateKey, 256); \
         return toHex(ab) === toHex(ba) ? 'agree' : 'disagree';",
    );
    assert_eq!(out, Value::String("agree".into()));
}

#[test]
fn subtle_ecdh_derive_key_then_aes_gcm_round_trips() {
    // ECDH deriveKey → AES-GCM, used end-to-end between two parties.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); \
         const algo = { name: 'ECDH', namedCurve: 'P-256' }; \
         const a = await crypto.subtle.generateKey(algo, true, ['deriveKey']); \
         const b = await crypto.subtle.generateKey(algo, true, ['deriveKey']); \
         const keyA = await crypto.subtle.deriveKey({ name: 'ECDH', public: b.publicKey }, a.privateKey, { name: 'AES-GCM', length: 256 }, false, ['encrypt']); \
         const keyB = await crypto.subtle.deriveKey({ name: 'ECDH', public: a.publicKey }, b.privateKey, { name: 'AES-GCM', length: 256 }, false, ['decrypt']); \
         const iv = crypto.getRandomValues(new Uint8Array(12)); \
         const ct = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, keyA, enc.encode('shared')); \
         const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv }, keyB, ct); \
         return new TextDecoder().decode(pt);",
    );
    assert_eq!(out, Value::String("shared".into()));
}

#[test]
fn subtle_rsa_all_schemes_and_formats() {
    // One 2048-bit key generation (the expensive step) reused across every
    // scheme and key format: PKCS1-v1_5 + PSS sign/verify, OAEP round-trip,
    // and SPKI/PKCS8/JWK export→import.
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const enc = new TextEncoder(); const data = enc.encode('rsa payload'); \
         const kp = await crypto.subtle.generateKey({ name: 'RSASSA-PKCS1-v1_5', modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: 'SHA-256' }, true, ['sign', 'verify']); \
         const sig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', kp.privateKey, data); \
         const good = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', kp.publicKey, sig, data); \
         const bad = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', kp.publicKey, sig, enc.encode('tampered')); \
         const pkcs8 = await crypto.subtle.exportKey('pkcs8', kp.privateKey); \
         const spki = await crypto.subtle.exportKey('spki', kp.publicKey); \
         const pssPriv = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'RSA-PSS', hash: 'SHA-256' }, false, ['sign']); \
         const pssPub = await crypto.subtle.importKey('spki', spki, { name: 'RSA-PSS', hash: 'SHA-256' }, true, ['verify']); \
         const pssSig = await crypto.subtle.sign({ name: 'RSA-PSS', saltLength: 32 }, pssPriv, data); \
         const pssOk = await crypto.subtle.verify({ name: 'RSA-PSS', saltLength: 32 }, pssPub, pssSig, data); \
         const oaepPriv = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'RSA-OAEP', hash: 'SHA-256' }, false, ['decrypt']); \
         const oaepPub = await crypto.subtle.importKey('spki', spki, { name: 'RSA-OAEP', hash: 'SHA-256' }, true, ['encrypt']); \
         const ct = await crypto.subtle.encrypt({ name: 'RSA-OAEP' }, oaepPub, enc.encode('secret')); \
         const pt = new TextDecoder().decode(await crypto.subtle.decrypt({ name: 'RSA-OAEP' }, oaepPriv, ct)); \
         const ctL = await crypto.subtle.encrypt({ name: 'RSA-OAEP', label: enc.encode('ctx') }, oaepPub, enc.encode('labeled')); \
         const ptL = new TextDecoder().decode(await crypto.subtle.decrypt({ name: 'RSA-OAEP', label: enc.encode('ctx') }, oaepPriv, ctL)); \
         const jwkPriv = await crypto.subtle.exportKey('jwk', kp.privateKey); \
         const jwkPub = await crypto.subtle.exportKey('jwk', kp.publicKey); \
         const fromJwkPriv = await crypto.subtle.importKey('jwk', jwkPriv, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['sign']); \
         const fromJwkPub = await crypto.subtle.importKey('jwk', jwkPub, { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['verify']); \
         const jwkSig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', fromJwkPriv, data); \
         const jwkOk = await crypto.subtle.verify('RSASSA-PKCS1-v1_5', fromJwkPub, jwkSig, data); \
         return [good === true, bad === false, pssOk === true, pt === 'secret', ptL === 'labeled', jwkOk === true].every(Boolean) ? 'all-ok' : 'mismatch';",
    );
    assert_eq!(out, Value::String("all-ok".into()));
}

#[test]
fn subtle_rsa_matches_an_independent_implementation() {
    // Known answers from OpenSSL (testdata/rsa-kat.json): what the runtime
    // produces and accepts must agree byte for byte with another
    // implementation, not only with itself (D203). PKCS#1 v1.5 signing is
    // deterministic, so its signature must be identical; PSS and OAEP are
    // randomised, so OpenSSL's must verify and decrypt here.
    let _g = v8_guard();
    let mut rt = runtime();
    let fixture = include_str!("../../testdata/rsa-kat.json");
    let script = format!(
        "const fx = {fixture}; \
         const b = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0)); \
         const same = (x, y) => x.length === y.length && x.every((v, i) => v === y[i]); \
         const subtle = crypto.subtle; \
         const msg = new TextEncoder().encode('known answer'); \
         const failed = []; \
         const check = (name, ok) => {{ if (!ok) failed.push(name); }}; \
         const rejects = async (name, run) => {{ try {{ await run(); failed.push(name); }} catch {{}} }}; \
         const pkcs1 = {{ name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }}; \
         const priv = await subtle.importKey('pkcs8', b(fx.pkcs8), pkcs1, true, ['sign']); \
         const pub = await subtle.importKey('spki', b(fx.spki), pkcs1, true, ['verify']); \
         check('pkcs1 verifies openssl', await subtle.verify(pkcs1, pub, b(fx.pkcs1), msg)); \
         const ours = new Uint8Array(await subtle.sign(pkcs1, priv, msg)); \
         check('pkcs1 signature is openssl\\'s', same(ours, b(fx.pkcs1))); \
         const tampered = b(fx.pkcs1); tampered[10] ^= 1; \
         check('pkcs1 rejects a tampered signature', !(await subtle.verify(pkcs1, pub, tampered, msg))); \
         const pss = {{ name: 'RSA-PSS', hash: 'SHA-256' }}; \
         const pssPub = await subtle.importKey('spki', b(fx.spki), pss, true, ['verify']); \
         check('pss verifies openssl', await subtle.verify({{ name: 'RSA-PSS', saltLength: 32 }}, pssPub, b(fx.pss), msg)); \
         check('pss rejects the wrong salt length', !(await subtle.verify({{ name: 'RSA-PSS', saltLength: 20 }}, pssPub, b(fx.pss), msg))); \
         const oaep = (hash) => subtle.importKey('pkcs8', b(fx.pkcs8), {{ name: 'RSA-OAEP', hash }}, false, ['decrypt']); \
         const text = (buf) => new TextDecoder().decode(buf); \
         const sha256 = await oaep('SHA-256'); \
         check('oaep decrypts openssl', text(await subtle.decrypt({{ name: 'RSA-OAEP' }}, sha256, b(fx.oaep))) === 'oaep secret'); \
         const binLabel = new Uint8Array([0xff, 0xfe, 0x00]); \
         check('oaep takes a non-UTF-8 label', text(await subtle.decrypt({{ name: 'RSA-OAEP', label: binLabel }}, sha256, b(fx.oaepBin))) === 'oaep secret'); \
         await rejects('oaep refuses the wrong label', () => subtle.decrypt({{ name: 'RSA-OAEP', label: new Uint8Array([1]) }}, sha256, b(fx.oaepBin))); \
         const sha1 = await oaep('SHA-1'); \
         check('oaep sha-1 with a label', text(await subtle.decrypt({{ name: 'RSA-OAEP', label: new TextEncoder().encode('ctx') }}, sha1, b(fx.oaepSha1))) === 'oaep secret'); \
         const corrupt = b(fx.oaep); corrupt[0] ^= 1; \
         await rejects('oaep refuses a corrupt ciphertext', () => subtle.decrypt({{ name: 'RSA-OAEP' }}, sha256, corrupt)); \
         const fromJwk = await subtle.importKey('jwk', fx.jwk, pkcs1, true, ['sign']); \
         check('a jwk-imported key signs as openssl', same(new Uint8Array(await subtle.sign(pkcs1, fromJwk, msg)), b(fx.pkcs1))); \
         const exported = await subtle.exportKey('jwk', priv); \
         for (const k of ['n', 'e', 'd', 'p', 'q', 'dp', 'dq', 'qi']) check(`jwk export ${{k}}`, exported[k] === fx.jwk[k]); \
         const roundTrip = new Uint8Array(await subtle.exportKey('pkcs8', fromJwk)); \
         check('jwk to pkcs8 signs alike', same(new Uint8Array(await subtle.sign(pkcs1, await subtle.importKey('pkcs8', roundTrip, pkcs1, false, ['sign']), msg)), b(fx.pkcs1))); \
         await rejects('a 512-bit private key is refused', () => subtle.importKey('pkcs8', b(fx.small), pkcs1, false, ['sign'])); \
         const smallPub = await (async () => {{ try {{ await subtle.importKey('pkcs8', b(fx.small), pkcs1, true, ['sign']); }} catch (e) {{ return e.name; }} }})(); \
         check('the refusal is a DataError', smallPub === 'DataError'); \
         await rejects('a 512-bit modulus is not generated', () => subtle.generateKey({{ ...pkcs1, modulusLength: 512, publicExponent: new Uint8Array([1, 0, 1]) }}, true, ['sign'])); \
         await rejects('a 9000-bit modulus is not generated', () => subtle.generateKey({{ ...pkcs1, modulusLength: 9000, publicExponent: new Uint8Array([1, 0, 1]) }}, true, ['sign'])); \
         return failed.length === 0 ? 'all-ok' : failed.join('; ');"
    );
    let out = eval_async(&mut rt, &script);
    assert_eq!(out, Value::String("all-ok".into()));
}

/// D256: numeric algorithm members are converted as WebIDL says, follow
/// the spec's length rules, and never make more than 64 KiB.
#[test]
fn subtle_lengths_follow_webidl_and_are_bounded() {
    let _g = v8_guard();
    let mut rt = runtime();
    let out = eval_async(
        &mut rt,
        "const subtle = crypto.subtle; \
         const failed = []; \
         const check = (name, ok) => { if (!ok) failed.push(name); }; \
         const fails = async (name, error, run) => { \
           try { await run(); failed.push(name + ': resolved'); } \
           catch (e) { if (e.name !== error) failed.push(`${name}: ${e.name} ${e.message}`); } \
         }; \
         const hmac = (length, hash = 'SHA-256') => subtle.generateKey({ name: 'HMAC', hash, length }, true, ['sign']); \
         const bits = async (key) => (await subtle.exportKey('raw', key)).byteLength * 8; \
         check('default is the block size', (await hmac(undefined)).algorithm.length === 512); \
         check('sha-512 default', (await hmac(undefined, 'SHA-512')).algorithm.length === 1024); \
         check('a fraction truncates', (await hmac(8.9)).algorithm.length === 8); \
         check('64 KiB is allowed', await bits(await hmac(65536 * 8)) === 65536 * 8); \
         await fails('zero', 'OperationError', () => hmac(0)); \
         await fails('past 64 KiB', 'OperationError', () => hmac(65536 * 8 + 1)); \
         await fails('2**32 - 1', 'OperationError', () => hmac(2 ** 32 - 1)); \
         await fails('2**32', 'TypeError', () => hmac(2 ** 32)); \
         await fails('negative', 'TypeError', () => hmac(-1)); \
         await fails('NaN', 'TypeError', () => hmac(NaN)); \
         await fails('aes needs a length', 'TypeError', () => subtle.generateKey({ name: 'AES-GCM' }, true, ['encrypt'])); \
         const raw = new Uint8Array(32).fill(7); \
         const imp = (length, data = raw) => subtle.importKey('raw', data, { name: 'HMAC', hash: 'SHA-256', length }, true, ['sign']); \
         check('import length', (await imp(252)).algorithm.length === 252); \
         check('import default', (await imp(undefined)).algorithm.length === 256); \
         await fails('import longer than data', 'DataError', () => imp(257)); \
         await fails('import a byte short', 'DataError', () => imp(248)); \
         await fails('import empty', 'DataError', () => imp(undefined, new Uint8Array(0))); \
         const pw = await subtle.importKey('raw', raw, 'PBKDF2', false, ['deriveBits', 'deriveKey']); \
         const pbkdf2 = { name: 'PBKDF2', hash: 'SHA-256', salt: raw, iterations: 1 }; \
         check('pbkdf2 64 KiB', (await subtle.deriveBits(pbkdf2, pw, 65536 * 8)).byteLength === 65536); \
         check('pbkdf2 zero length', (await subtle.deriveBits(pbkdf2, pw, 0)).byteLength === 0); \
         await fails('pbkdf2 past 64 KiB', 'OperationError', () => subtle.deriveBits(pbkdf2, pw, 65536 * 8 + 8)); \
         await fails('pbkdf2 4 GiB', 'OperationError', () => subtle.deriveBits(pbkdf2, pw, 2 ** 32 - 8)); \
         await fails('deriveBits length 2**32', 'TypeError', () => subtle.deriveBits(pbkdf2, pw, 2 ** 32)); \
         await fails('pbkdf2 iterations -1', 'TypeError', () => subtle.deriveBits({ ...pbkdf2, iterations: -1 }, pw, 256)); \
         const derived = await subtle.deriveKey(pbkdf2, pw, { name: 'HMAC', hash: 'SHA-256' }, true, ['sign']); \
         check('derived hmac default', derived.algorithm.length === 512); \
         await fails('derived hmac zero', 'TypeError', () => subtle.deriveKey(pbkdf2, pw, { name: 'HMAC', hash: 'SHA-256', length: 0 }, true, ['sign'])); \
         const ikm = await subtle.importKey('raw', raw, 'HKDF', false, ['deriveBits']); \
         await fails('hkdf past its limit', 'OperationError', () => subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: raw, info: raw }, ikm, 2 ** 32 - 8)); \
         const aes = await subtle.generateKey({ name: 'AES-GCM', length: 128 }, true, ['encrypt']); \
         const iv = new Uint8Array(12); \
         check('gcm 128-bit tag', (await subtle.encrypt({ name: 'AES-GCM', iv, tagLength: 128 }, aes, raw)).byteLength === 48); \
         await fails('gcm 96-bit tag', 'NotSupportedError', () => subtle.encrypt({ name: 'AES-GCM', iv, tagLength: 96 }, aes, raw)); \
         await fails('gcm 100-bit tag', 'OperationError', () => subtle.encrypt({ name: 'AES-GCM', iv, tagLength: 100 }, aes, raw)); \
         await fails('gcm 256-bit tag', 'TypeError', () => subtle.encrypt({ name: 'AES-GCM', iv, tagLength: 256 }, aes, raw)); \
         const algorithm = { name: 'HMAC', hash: 'SHA-256', length: 8.5 }; \
         await subtle.generateKey(algorithm, true, ['sign']); \
         check('the caller object is not changed', algorithm.length === 8.5); \
         return failed.length === 0 ? 'all-ok' : failed.join('; ');",
    );
    assert_eq!(out, Value::String("all-ok".into()));
}

/// The bound holds at the op as well, for any caller (D256): the `ops`
/// fuzz target once made it allocate 4 GB.
#[test]
fn random_bytes_op_draws_at_most_64_kib() {
    let _g = v8_guard();
    let mut rt = runtime();
    rt.expose_ops_for_testing();
    let out = eval_async(
        &mut rt,
        "const ok = __ops.random_bytes(65536).length === 65536; \
         let refused = ''; \
         try { __ops.random_bytes(2 ** 32 - 1); } catch (e) { refused = e.name; } \
         return `${ok} ${refused}`;",
    );
    assert_eq!(out, Value::String("true RangeError".into()));
}
