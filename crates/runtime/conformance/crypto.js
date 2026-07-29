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
