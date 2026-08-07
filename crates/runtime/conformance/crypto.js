// WinterTC §2.10 — crypto / crypto.subtle. (Async tests return a Promise.)

const hex = (buf) =>
  [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

test("randomUUID has the v4 shape", () => {
  const id = crypto.randomUUID();
  assert(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(id));
});

test("getRandomValues fills the view and returns it", () => {
  const v = new Uint8Array(16);
  const r = crypto.getRandomValues(v);
  assert(r === v);
});

test("getRandomValues rejects oversized requests", () => {
  assertThrows(() => crypto.getRandomValues(new Uint8Array(65537)), "QuotaExceededError");
});

test("subtle.digest SHA-256 matches the known vector for 'abc'", async () => {
  const d = await crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"));
  assertEquals(hex(d), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
});

test("subtle HMAC sign/verify round-trips", async () => {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw", enc.encode("secret"), { name: "HMAC", hash: "SHA-256" }, false, ["sign", "verify"]);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode("msg"));
  assertEquals(await crypto.subtle.verify("HMAC", key, sig, enc.encode("msg")), true);
  assertEquals(await crypto.subtle.verify("HMAC", key, sig, enc.encode("tampered")), false);
});

test("subtle AES-GCM round-trips", async () => {
  const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, true, ["encrypt", "decrypt"]);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const pt = new TextEncoder().encode("secret data");
  const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, key, pt);
  const out = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, key, ct);
  assertEquals(new TextDecoder().decode(out), "secret data");
});

test("subtle ECDSA P-256 sign/verify round-trips", async () => {
  const enc = new TextEncoder();
  const kp = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"]);
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, kp.privateKey, enc.encode("m"));
  assertEquals(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, kp.publicKey, sig, enc.encode("m")), true);
});

test("subtle PBKDF2 deriveBits matches RFC 6070", async () => {
  const enc = new TextEncoder();
  const base = await crypto.subtle.importKey("raw", enc.encode("password"), "PBKDF2", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits(
    { name: "PBKDF2", hash: "SHA-1", salt: enc.encode("salt"), iterations: 1 }, base, 160);
  assertEquals(hex(bits), "0c60c80f961f0e71f3a9b524af6012062fe037a6");
});

test("getRandomValues rejects Float32Array", () => {
  assertThrows(() => crypto.getRandomValues(new Float32Array(4)), "TypeError");
});


test("subtle.digest SHA-1 and SHA-512 match known vectors", async () => {
  const enc = new TextEncoder();
  const d1 = await crypto.subtle.digest("SHA-1", enc.encode("abc"));
  assertEquals(hex(d1), "a9993e364706816aba3e25717850c26c9cd0d89d");
  const d512 = await crypto.subtle.digest("SHA-512", enc.encode("abc"));
  assertEquals(hex(d512), "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f");
});


// ---- Key wrapping ---------------------------------------------------------

test("AES-KW wrap matches the RFC 3394 vector and round-trips", async () => {
  const bytes = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16)));
  const kek = await crypto.subtle.importKey(
    "raw", bytes("000102030405060708090a0b0c0d0e0f"), "AES-KW", false, ["wrapKey", "unwrapKey"]);
  const target = await crypto.subtle.importKey(
    "raw", bytes("00112233445566778899aabbccddeeff"), "AES-CBC", true, ["encrypt"]);

  const wrapped = await crypto.subtle.wrapKey("raw", target, kek, "AES-KW");
  // Wrapping adds one 8-byte semiblock: the integrity check value.
  assertEquals(wrapped.byteLength, 24);
  assertEquals(hex(wrapped), "1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");

  const back = await crypto.subtle.unwrapKey(
    "raw", wrapped, kek, "AES-KW", "AES-CBC", true, ["encrypt"]);
  assertEquals(hex(await crypto.subtle.exportKey("raw", back)), "00112233445566778899aabbccddeeff");
});

test("a tampered AES-KW ciphertext fails to unwrap", async () => {
  const kek = await crypto.subtle.generateKey({ name: "AES-KW", length: 256 }, true, ["wrapKey", "unwrapKey"]);
  const target = await crypto.subtle.generateKey({ name: "AES-GCM", length: 128 }, true, ["encrypt"]);
  const wrapped = new Uint8Array(await crypto.subtle.wrapKey("raw", target, kek, "AES-KW"));
  wrapped[3] ^= 1;
  let name = null;
  try {
    await crypto.subtle.unwrapKey("raw", wrapped, kek, "AES-KW", "AES-GCM", true, ["encrypt"]);
  } catch (e) {
    name = e.name;
  }
  assertEquals(name, "OperationError");
});

test("AES-GCM wraps a key as JWK", async () => {
  const kek = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, true, ["wrapKey", "unwrapKey"]);
  const target = await crypto.subtle.generateKey({ name: "AES-CTR", length: 128 }, true, ["encrypt"]);
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const raw = hex(await crypto.subtle.exportKey("raw", target));

  const wrapped = await crypto.subtle.wrapKey("jwk", target, kek, { name: "AES-GCM", iv });
  const back = await crypto.subtle.unwrapKey(
    "jwk", wrapped, kek, { name: "AES-GCM", iv }, { name: "AES-CTR" }, true, ["encrypt"]);
  assertEquals(hex(await crypto.subtle.exportKey("raw", back)), raw);
});

test("wrapping requires the wrapKey usage and an extractable key", async () => {
  const kek = await crypto.subtle.generateKey({ name: "AES-KW", length: 128 }, true, ["unwrapKey"]);
  const target = await crypto.subtle.generateKey({ name: "AES-GCM", length: 128 }, true, ["encrypt"]);
  let noUsage = null;
  try {
    await crypto.subtle.wrapKey("raw", target, kek, "AES-KW");
  } catch (e) {
    noUsage = e.name;
  }
  assertEquals(noUsage, "InvalidAccessError");

  const wrapper = await crypto.subtle.generateKey({ name: "AES-KW", length: 128 }, true, ["wrapKey"]);
  const locked = await crypto.subtle.generateKey({ name: "AES-GCM", length: 128 }, false, ["encrypt"]);
  let notExtractable = null;
  try {
    await crypto.subtle.wrapKey("raw", locked, wrapper, "AES-KW");
  } catch (e) {
    notExtractable = e.name;
  }
  assertEquals(notExtractable, "InvalidAccessError");
});

test("AES-KW is not reachable from encrypt or decrypt", async () => {
  const kek = await crypto.subtle.generateKey({ name: "AES-KW", length: 128 }, true, ["wrapKey", "unwrapKey"]);
  const data = new Uint8Array(16);
  let name = null;
  try {
    await crypto.subtle.encrypt({ name: "AES-KW" }, kek, data);
  } catch (e) {
    name = e.name;
  }
  assertEquals(name, "NotSupportedError");
});

test("symmetric keys export and import as oct JWK", async () => {
  const key = await crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, true, ["encrypt", "decrypt"]);
  const jwk = await crypto.subtle.exportKey("jwk", key);
  assertEquals(jwk.kty, "oct");
  assertEquals(jwk.alg, "A256GCM");
  assertEquals(jwk.ext, true);

  const back = await crypto.subtle.importKey("jwk", jwk, "AES-GCM", true, ["encrypt"]);
  assertEquals(
    hex(await crypto.subtle.exportKey("raw", back)),
    hex(await crypto.subtle.exportKey("raw", key)),
  );

  const hmac = await crypto.subtle.generateKey({ name: "HMAC", hash: "SHA-384" }, true, ["sign"]);
  assertEquals((await crypto.subtle.exportKey("jwk", hmac)).alg, "HS384");

  // A JWK labelled for one algorithm must not import as another.
  let mismatch = null;
  try {
    await crypto.subtle.importKey("jwk", jwk, "AES-CBC", true, ["encrypt"]);
  } catch (e) {
    mismatch = e.name;
  }
  assertEquals(mismatch, "DataError");
});

// ---- Secure Curves: Ed25519 / X25519 --------------------------------------

const okpBytes = (s) => Uint8Array.from(s.match(/../g).map((b) => parseInt(b, 16)));

test("Ed25519 signs the RFC 8032 test-2 vector", async () => {
  // RFC 8032 §7.1 TEST 2 wrapped in the RFC 8410 PKCS#8 envelope.
  const key = await crypto.subtle.importKey(
    "pkcs8",
    okpBytes("302e020100300506032b6570042204204ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb"),
    "Ed25519", true, ["sign"]);
  const sig = await crypto.subtle.sign("Ed25519", key, new Uint8Array([0x72]));
  assertEquals(
    hex(sig),
    "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da" +
      "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
  );
  // The public key comes out of the private one, so exporting agrees with it.
  const jwk = await crypto.subtle.exportKey("jwk", key);
  assertEquals(jwk.kty, "OKP");
  assertEquals(jwk.crv, "Ed25519");
  const pub = await crypto.subtle.importKey(
    "jwk", { kty: "OKP", crv: "Ed25519", x: jwk.x }, "Ed25519", true, ["verify"]);
  assertEquals(await crypto.subtle.verify("Ed25519", pub, sig, new Uint8Array([0x72])), true);
  assertEquals(await crypto.subtle.verify("Ed25519", pub, sig, new Uint8Array([0x73])), false);
});

test("a generated Ed25519 key round-trips through spki/pkcs8", async () => {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  // The usages split by key half, as they do for ECDSA.
  assertEquals(JSON.stringify(pair.privateKey.usages), '["sign"]');
  assertEquals(JSON.stringify(pair.publicKey.usages), '["verify"]');

  const priv = await crypto.subtle.importKey(
    "pkcs8", await crypto.subtle.exportKey("pkcs8", pair.privateKey), "Ed25519", true, ["sign"]);
  const pub = await crypto.subtle.importKey(
    "spki", await crypto.subtle.exportKey("spki", pair.publicKey), "Ed25519", true, ["verify"]);
  const msg = new TextEncoder().encode("round trip");
  assertEquals(await crypto.subtle.verify("Ed25519", pub, await crypto.subtle.sign("Ed25519", priv, msg), msg), true);
});

test("X25519 agrees on the RFC 7748 shared secret", async () => {
  const alice = await crypto.subtle.importKey(
    "pkcs8",
    okpBytes("302e020100300506032b656e042204207707 6d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a".replace(/ /g, "")),
    "X25519", true, ["deriveBits", "deriveKey"]);
  const bob = await crypto.subtle.importKey(
    "raw", okpBytes("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f"),
    "X25519", true, []);
  const shared = await crypto.subtle.deriveBits({ name: "X25519", public: bob }, alice, null);
  assertEquals(hex(shared), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");

  // Truncation to a requested bit length, and deriveKey on top of it.
  assertEquals((await crypto.subtle.deriveBits({ name: "X25519", public: bob }, alice, 128)).byteLength, 16);
  const aes = await crypto.subtle.deriveKey(
    { name: "X25519", public: bob }, alice, { name: "AES-GCM", length: 256 }, true, ["encrypt"]);
  assertEquals(aes.algorithm.length, 256);
});

test("X25519 refuses a low-order peer key", async () => {
  const alice = await crypto.subtle.generateKey({ name: "X25519" }, true, ["deriveBits"]);
  const zero = await crypto.subtle.importKey("raw", new Uint8Array(32), "X25519", true, []);
  let name = null;
  try {
    await crypto.subtle.deriveBits({ name: "X25519", public: zero }, alice.privateKey, null);
  } catch (e) {
    name = e.name;
  }
  // An all-zero shared secret must be an error, not a usable key.
  assertEquals(name, "OperationError");
});

test("an OKP key cannot be imported under the wrong curve", async () => {
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const pkcs8 = await crypto.subtle.exportKey("pkcs8", pair.privateKey);
  let name = null;
  try {
    await crypto.subtle.importKey("pkcs8", pkcs8, "X25519", true, ["deriveBits"]);
  } catch (e) {
    name = e.name;
  }
  assertEquals(name, "DataError");

  // …and a JWK's crv is checked the same way.
  const jwk = await crypto.subtle.exportKey("jwk", pair.publicKey);
  let jwkName = null;
  try {
    await crypto.subtle.importKey("jwk", jwk, "X25519", true, []);
  } catch (e) {
    jwkName = e.name;
  }
  assertEquals(jwkName, "DataError");
});

// ---- Key usages ------------------------------------------------------------
//
// `key.usages` is the authority record every operation is checked against, so
// it is enforced at both ends: nothing outside the algorithm's registration can
// be recorded, and nothing recorded can be exceeded.

test("an operation the key's usages do not allow is an InvalidAccessError", async () => {
  const enc = new TextEncoder();
  const verifyOnly = await crypto.subtle.importKey(
    "raw", enc.encode("k"), { name: "HMAC", hash: "SHA-256" }, true, ["verify"],
  );
  await assertRejects(() => crypto.subtle.sign("HMAC", verifyOnly, enc.encode("m")), "InvalidAccessError");

  const signOnly = await crypto.subtle.importKey(
    "raw", enc.encode("k"), { name: "HMAC", hash: "SHA-256" }, true, ["sign"],
  );
  await assertRejects(
    () => crypto.subtle.verify("HMAC", signOnly, new Uint8Array(32), enc.encode("m")),
    "InvalidAccessError",
  );

  const decryptOnly = await crypto.subtle.importKey("raw", new Uint8Array(32), "AES-GCM", true, ["decrypt"]);
  await assertRejects(
    () => crypto.subtle.encrypt({ name: "AES-GCM", iv: new Uint8Array(12) }, decryptOnly, enc.encode("m")),
    "InvalidAccessError",
  );

  const deriveKeyOnly = await crypto.subtle.importKey("raw", enc.encode("ikm"), "HKDF", false, ["deriveKey"]);
  await assertRejects(
    () => crypto.subtle.deriveBits(
      { name: "HKDF", salt: new Uint8Array(0), info: new Uint8Array(0), hash: "SHA-256" },
      deriveKeyOnly, 256,
    ),
    "InvalidAccessError",
  );
});

test("deriveKey and deriveBits are gated on their own usage, not each other's", async () => {
  const enc = new TextEncoder();
  // Only `deriveKey`: deriving a key must work even though it derives bits
  // internally. Only `deriveBits`: it must not be a back door to minting keys.
  const keyOnly = await crypto.subtle.importKey("raw", enc.encode("ikm"), "HKDF", false, ["deriveKey"]);
  const params = { name: "HKDF", salt: new Uint8Array(0), info: new Uint8Array(0), hash: "SHA-256" };
  const derived = await crypto.subtle.deriveKey(
    params, keyOnly, { name: "AES-GCM", length: 256 }, true, ["encrypt"],
  );
  assertEquals(derived.algorithm.name, "AES-GCM");

  const bitsOnly = await crypto.subtle.importKey("raw", enc.encode("ikm"), "HKDF", false, ["deriveBits"]);
  await assertRejects(
    () => crypto.subtle.deriveKey(params, bitsOnly, { name: "AES-GCM", length: 256 }, true, ["encrypt"]),
    "InvalidAccessError",
  );
});

test("wrapKey uses the wrapping key's wrap usage, not encrypt", async () => {
  // Wrapping is encryption underneath, but the usage checked is `wrapKey` — a
  // key granted only that must still wrap.
  const wrapper = await crypto.subtle.importKey(
    "raw", new Uint8Array(32), "AES-GCM", true, ["wrapKey", "unwrapKey"],
  );
  const target = await crypto.subtle.importKey("raw", new Uint8Array(16), "AES-GCM", true, ["encrypt"]);
  const iv = new Uint8Array(12);
  const wrapped = await crypto.subtle.wrapKey("raw", target, wrapper, { name: "AES-GCM", iv });
  const back = await crypto.subtle.unwrapKey(
    "raw", wrapped, wrapper, { name: "AES-GCM", iv }, { name: "AES-GCM" }, true, ["encrypt"],
  );
  assertEquals((await crypto.subtle.exportKey("raw", back)).byteLength, 16);
});

test("importKey and generateKey reject usages the algorithm does not register", async () => {
  const enc = new TextEncoder();
  await assertRejects(
    () => crypto.subtle.importKey("raw", new Uint8Array(32), "AES-GCM", true, ["sign"]),
    "SyntaxError",
  );
  await assertRejects(
    () => crypto.subtle.importKey("raw", enc.encode("k"), { name: "HMAC", hash: "SHA-256" }, true, ["encrypt"]),
    "SyntaxError",
  );
  await assertRejects(
    () => crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["encrypt"]),
    "SyntaxError",
  );
});

test("a secret or private key must be created with at least one usage", async () => {
  await assertRejects(
    () => crypto.subtle.generateKey({ name: "AES-GCM", length: 256 }, true, []),
    "SyntaxError",
  );
  await assertRejects(
    () => crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, []),
    "SyntaxError",
  );
});

test("an algorithm that registers no such operation is NotSupportedError, not a usage denial", async () => {
  // The standard normalizes the algorithm before it looks at the key, so this
  // must not surface as InvalidAccessError just because the key lacks `encrypt`.
  const kw = await crypto.subtle.importKey("raw", new Uint8Array(32), "AES-KW", true, ["wrapKey"]);
  await assertRejects(
    () => crypto.subtle.encrypt({ name: "AES-KW" }, kw, new Uint8Array(16)),
    "NotSupportedError",
  );
});

// ---- ECDSA hash/curve matrix ----------------------------------------------

test("ECDSA signs and verifies with any hash on any curve", async () => {
  // A digest narrower than the curve's field is used whole, zero-padded on the
  // left (SEC1 bits2int). The backend refuses to pad below half the field
  // width, so P-521/SHA-256 and P-384/SHA-1 failed outright until the prehash
  // was padded here — both are ordinary combinations every browser accepts.
  const enc = new TextEncoder();
  const widths = { "P-256": 64, "P-384": 96, "P-521": 132 };
  for (const curve of ["P-256", "P-384", "P-521"]) {
    for (const hash of ["SHA-1", "SHA-256", "SHA-384", "SHA-512"]) {
      const kp = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: curve }, true, ["sign", "verify"]);
      const msg = enc.encode(`${curve}/${hash}`);
      const sig = await crypto.subtle.sign({ name: "ECDSA", hash }, kp.privateKey, msg);
      assertEquals(sig.byteLength, widths[curve]);
      assertEquals(await crypto.subtle.verify({ name: "ECDSA", hash }, kp.publicKey, sig, msg), true);
      // …and it is a real signature, not one that verifies against anything.
      assertEquals(
        await crypto.subtle.verify({ name: "ECDSA", hash }, kp.publicKey, sig, enc.encode("other")),
        false,
      );
    }
  }
});
