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
