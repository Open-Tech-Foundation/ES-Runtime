//! End-to-end tests for `runtime:hashing` (DECISIONS D57).
//!
//! These run the real `esrun` binary, so every assertion crosses the whole
//! path: the baked ES module, the ops, and the RustCrypto backends underneath.
//! The vectors themselves are checked in `hashing_ops`' unit tests; what is
//! checked here is that the module hands them across unchanged, that the
//! encodings and input types behave, and — the reason it is an end-to-end test
//! and not a unit test — that none of it needs a capability.

use std::path::PathBuf;
use std::process::{Command, Output};

fn temp(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name)
}

fn write(name: &str, contents: &str) -> PathBuf {
    let path = temp(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esrun() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_esrun"));
    // Run *from* the directory these fixtures are written into: the sandbox is
    // the working directory (D79), so a program is run from where it lives.
    command.current_dir(env!("CARGO_TARGET_TMPDIR"));
    command
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `source` and returns its stdout, failing with the child's stderr if it
/// did not exit cleanly. A thrown assertion inside the script is a non-zero
/// exit, so the JS can assert for itself.
fn run(name: &str, source: &str) -> String {
    let app = write(name, source);
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout(&out)
}

/// The same, with every capability withheld.
fn run_denied(name: &str, source: &str) -> String {
    let app = write(name, source);
    let out = esrun().arg("--deny-all").arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    stdout(&out)
}

/// A helper the assertions below share: a script prelude with `eq`.
const EQ: &str = r#"
const eq = (actual, expected, what) => {
  if (actual !== expected) throw new Error(`${what}: expected ${expected}, got ${actual}`);
};
"#;

/// Every algorithm, hex, through the real module. The published vectors are the
/// unit tests' business; this is the wiring, all fifteen of them.
#[test]
fn every_algorithm_hashes_through_the_module() {
    let s = run(
        "hash_all.mjs",
        &format!(
            r#"
import {{ hash }} from "runtime:hashing";
{EQ}
eq(hash("sha1", "abc", "hex"), "a9993e364706816aba3e25717850c26c9cd0d89d", "sha1");
eq(hash("sha256", "abc", "hex"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "sha256");
eq(hash("sha384", "abc", "hex"), "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7", "sha384");
eq(hash("sha512", "abc", "hex"), "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f", "sha512");
eq(hash("sha3-224", "abc", "hex"), "e642824c3f8cf24ad09234ee7d3c766fc9a3a5168d0c94ad73b46fdf", "sha3-224");
eq(hash("sha3-256", "abc", "hex"), "3a985da74fe225b2045c172d6bd390bd855f086e3e9d525b46bfe24511431532", "sha3-256");
eq(hash("sha3-384", "abc", "hex"), "ec01498288516fc926459f58e2c6ad8df9b473cb0fc08c2596da7cf0e49be4b298d88cea927ac7f539f1edf228376d25", "sha3-384");
eq(hash("sha3-512", "abc", "hex"), "b751850b1a57168a5693cd924b6b096e08f621827444f70d884f5d0240d2712e10e116e9192af3c91a7ec57647e3934057340b4cf408d5a56592f8274eec53f0", "sha3-512");
eq(hash("blake3", "abc", "hex"), "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85", "blake3");
eq(hash("md5", "abc", "hex"), "900150983cd24fb0d6963f7d28e17f72", "md5");
eq(hash("ripemd160", "abc", "hex"), "8eb208f7e05d987a9b044a8e98c6b087f15a0bfc", "ripemd160");
eq(hash("xxhash64", "", "hex"), "ef46db3751d8e999", "xxhash64");
eq(hash("xxhash3", "", "hex"), "2d06800538d394c2", "xxhash3");
eq(hash("crc32", "123456789", "hex"), "cbf43926", "crc32");
eq(hash("crc32c", "123456789", "hex"), "e3069283", "crc32c");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The same digest, every way it can leave the host — and bytes by default, so
/// the common case needs no third argument.
#[test]
fn every_encoding_encodes_the_same_digest() {
    let s = run(
        "hash_encodings.mjs",
        &format!(
            r#"
import {{ hash }} from "runtime:hashing";
{EQ}
const hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
eq(hash("sha256", "abc", "hex"), hex, "hex");
eq(hash("sha256", "abc", "base64"), "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=", "base64");
eq(hash("sha256", "abc", "base64url"), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0", "base64url");

const bytes = hash("sha256", "abc");
eq(bytes.constructor.name, "Uint8Array", "default is bytes");
eq(bytes.length, 32, "length");
eq([...bytes].map((b) => b.toString(16).padStart(2, "0")).join(""), hex, "bytes match hex");
eq(hash("sha256", "abc", "bytes").length, 32, "explicit bytes");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// Every way an input can arrive is the same input. A string is its UTF-8
/// bytes, and a view's offset is respected — a `subarray` must hash as itself,
/// not as the buffer it points into.
#[test]
fn strings_buffers_and_views_hash_alike() {
    let s = run(
        "hash_inputs.mjs",
        &format!(
            r#"
import {{ hash }} from "runtime:hashing";
{EQ}
const hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const bytes = new TextEncoder().encode("abc");
eq(hash("sha256", "abc", "hex"), hex, "string");
eq(hash("sha256", bytes, "hex"), hex, "Uint8Array");
eq(hash("sha256", bytes.buffer, "hex"), hex, "ArrayBuffer");
eq(hash("sha256", new DataView(bytes.buffer), "hex"), hex, "DataView");

const padded = new TextEncoder().encode("xxabcxx");
eq(hash("sha256", padded.subarray(2, 5), "hex"), hex, "offset view");

// Non-ASCII is UTF-8, the same bytes TextEncoder would have produced.
eq(hash("sha256", "héllo 🌍", "hex"), hash("sha256", new TextEncoder().encode("héllo 🌍"), "hex"), "utf-8");

let threw = false;
try {{ hash("sha256", 42); }} catch (e) {{ threw = e instanceof TypeError; }}
eq(threw, true, "a number is refused");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The point of the incremental API: chunked input must equal whole input, for
/// every algorithm, and a hasher must be finished exactly once.
#[test]
fn a_hasher_matches_the_one_shot_and_ends_after_its_digest() {
    let s = run(
        "hash_incremental.mjs",
        &format!(
            r#"
import {{ hash, Hasher }} from "runtime:hashing";
{EQ}
const algorithms = ["sha1", "sha256", "sha384", "sha512", "sha3-224", "sha3-256",
  "sha3-384", "sha3-512", "blake3", "md5", "ripemd160", "xxhash64", "xxhash3",
  "crc32", "crc32c"];

const data = "the quick brown fox jumps over the lazy dog, repeatedly, ".repeat(40);
for (const algorithm of algorithms) {{
  const h = new Hasher(algorithm);
  for (let i = 0; i < data.length; i += 7) h.update(data.slice(i, i + 7));
  eq(h.digest("hex"), hash(algorithm, data, "hex"), algorithm);
}}

// Chaining, the empty hash, and the algorithm read back.
const chained = new Hasher("sha256");
eq(chained.algorithm, "sha256", "algorithm");
eq(chained.update("a").update("b").update("c").digest("hex"), hash("sha256", "abc", "hex"), "chained");
eq(new Hasher("sha256").digest("hex"), hash("sha256", "", "hex"), "empty");

// Once finished, it is finished — both ways.
const spent = new Hasher("sha256");
spent.digest();
for (const call of [() => spent.update("x"), () => spent.digest()]) {{
  let threw = false;
  try {{ call(); }} catch {{ threw = true; }}
  eq(threw, true, "a spent hasher throws");
}}
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The case the incremental API exists for, in the shape it is actually used:
/// a body arriving in chunks, hashed without being held.
#[test]
fn a_stream_is_hashed_as_it_arrives() {
    let s = run(
        "hash_stream.mjs",
        &format!(
            r#"
import {{ hash, hashStream }} from "runtime:hashing";
{EQ}
const chunks = ["the quick ", "brown fox ", "jumps over the lazy dog"];
const encoder = new TextEncoder();
const stream = new ReadableStream({{
  start(controller) {{
    for (const c of chunks) controller.enqueue(encoder.encode(c));
    controller.close();
  }},
}});
eq(await hashStream("sha256", stream, "hex"), hash("sha256", chunks.join(""), "hex"), "stream");

// The realistic caller: a Response body, hashed straight through.
const body = new Response("payload").body;
eq(await hashStream("blake3", body, "hex"), hash("blake3", "payload", "hex"), "response body");

const empty = new ReadableStream({{ start: (c) => c.close() }});
eq(await hashStream("sha256", empty, "hex"), hash("sha256", "", "hex"), "empty stream");

let threw = false;
try {{ await hashStream("sha256", "not a stream"); }} catch (e) {{ threw = e instanceof TypeError; }}
eq(threw, true, "a non-stream is refused");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// HMAC against its RFC vectors, and the two refusals: a checksum cannot key a
/// MAC, and BLAKE3 has its own keyed mode rather than this one.
#[test]
fn hmac_matches_its_vectors_and_refuses_what_is_not_a_hash() {
    let s = run(
        "hash_hmac.mjs",
        &format!(
            r#"
import {{ hmac }} from "runtime:hashing";
{EQ}
const key = "Jefe", data = "what do ya want for nothing?";
eq(hmac("sha1", key, data, "hex"), "effcdf6ae5eb2fa2d27416d5f184df9c259a7c79", "hmac-sha1");
eq(hmac("sha256", key, data, "hex"), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843", "hmac-sha256");
eq(hmac("md5", key, data, "hex"), "750c783e6ab0b503eaa86e310a5db738", "hmac-md5");
eq(hmac("sha256", key, data).length, 32, "bytes by default");

// Agrees with crypto.subtle, which is the same construction under another name.
const imported = await crypto.subtle.importKey(
  "raw", new TextEncoder().encode(key), {{ name: "HMAC", hash: "SHA-256" }}, false, ["sign"]);
const viaSubtle = new Uint8Array(await crypto.subtle.sign("HMAC", imported, new TextEncoder().encode(data)));
eq([...viaSubtle].map((b) => b.toString(16).padStart(2, "0")).join(""), hmac("sha256", key, data, "hex"), "subtle agrees");

for (const algorithm of ["crc32", "crc32c", "xxhash64", "xxhash3", "blake3"]) {{
  let threw = false;
  try {{ hmac(algorithm, "k", "d"); }} catch (e) {{ threw = e instanceof TypeError; }}
  eq(threw, true, `hmac refuses ${{algorithm}}`);
}}
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// `subtle.digest`'s spellings must work here too, or the two APIs disagree
/// about what a hash is called.
#[test]
fn webcrypto_algorithm_names_are_accepted() {
    let s = run(
        "hash_names.mjs",
        &format!(
            r#"
import {{ hash }} from "runtime:hashing";
{EQ}
const hex = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
for (const name of ["sha256", "SHA-256", "sha-256", "SHA256", "Sha256"]) {{
  eq(hash(name, "abc", "hex"), hex, name);
}}
eq(hash("SHA3-512", "abc", "hex"), hash("sha3-512", "abc", "hex"), "SHA3-512");

// And an unknown one is refused with the list of known ones.
let message = "";
try {{ hash("sha2", "abc"); }} catch (e) {{ message = e.message; }}
if (!message.includes("sha256") || !message.includes("blake3")) {{
  throw new Error(`unhelpful message: ${{message}}`);
}}
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The signature-checking case the export exists for.
#[test]
fn timing_safe_equal_compares_without_leaking_the_prefix() {
    let s = run(
        "hash_equal.mjs",
        &format!(
            r#"
import {{ hash, hmac, timingSafeEqual }} from "runtime:hashing";
{EQ}
eq(timingSafeEqual("abc", "abc"), true, "equal strings");
eq(timingSafeEqual("abc", "abd"), false, "different strings");
eq(timingSafeEqual("abc", "ab"), false, "different lengths");
eq(timingSafeEqual(new Uint8Array([1, 2, 3]), new Uint8Array([1, 2, 3])), true, "equal bytes");
eq(timingSafeEqual(hash("sha256", "x"), hash("sha256", "x")), true, "equal digests");
eq(timingSafeEqual(hash("sha256", "x"), hash("sha256", "y")), false, "different digests");

// A webhook signature check, written the way it should be.
const secret = "whsec_test", body = '{{"event":"ping"}}';
const header = hmac("sha256", secret, body, "hex");
eq(timingSafeEqual(header, hmac("sha256", secret, body, "hex")), true, "signature accepted");
eq(timingSafeEqual(header, hmac("sha256", "wrong", body, "hex")), false, "forgery rejected");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// Every password algorithm, round-tripped through the module, at costs low
/// enough for a test suite.
#[test]
fn every_password_algorithm_hashes_and_verifies() {
    let s = run(
        "hash_password.mjs",
        &format!(
            r#"
import {{ password }} from "runtime:hashing";
{EQ}
const cases = [
  ["argon2id", {{ memoryCost: 64, timeCost: 1 }}, "$argon2id$"],
  ["argon2i", {{ memoryCost: 64, timeCost: 1 }}, "$argon2i$"],
  ["argon2d", {{ memoryCost: 64, timeCost: 1 }}, "$argon2d$"],
  ["bcrypt", {{ cost: 4 }}, "$2b$04$"],
  ["scrypt", {{ cost: 10 }}, "$scrypt$"],
];
for (const [algorithm, options, prefix] of cases) {{
  const stored = await password.hash("correct horse battery staple", {{ algorithm, ...options }});
  if (!stored.startsWith(prefix)) throw new Error(`${{algorithm}}: ${{stored}}`);
  eq(await password.verify("correct horse battery staple", stored), true, `${{algorithm}} verifies`);
  eq(await password.verify("Correct horse battery staple", stored), false, `${{algorithm}} rejects`);
  eq(await password.verify("", stored), false, `${{algorithm}} rejects empty`);

  // A second hash of the same password differs: the salt is fresh each time.
  const again = await password.hash("correct horse battery staple", {{ algorithm, ...options }});
  if (again === stored) throw new Error(`${{algorithm}} reused its salt`);
  eq(await password.verify("correct horse battery staple", again), true, `${{algorithm}} verifies again`);
}}

// The default is Argon2id, and needs no algorithm named.
const dflt = await password.hash("pw", {{ memoryCost: 64, timeCost: 1 }});
if (!dflt.startsWith("$argon2id$")) throw new Error(dflt);
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The upgrade path: an old hash still verifies, and says it wants replacing.
/// Without this a raised cost would lock every existing user out.
#[test]
fn an_old_hash_verifies_and_reports_that_it_needs_rehashing() {
    let s = run(
        "hash_rehash.mjs",
        &format!(
            r#"
import {{ password }} from "runtime:hashing";
{EQ}
const weak = await password.hash("pw", {{ memoryCost: 64, timeCost: 1 }});
eq(await password.verify("pw", weak), true, "old hash still verifies");
eq(password.needsRehash(weak), true, "old parameters want rehashing");
eq(password.needsRehash(weak, {{ memoryCost: 64, timeCost: 1 }}), false, "same parameters do not");
eq(password.needsRehash(weak, {{ memoryCost: 32, timeCost: 1 }}), false, "stronger than asked does not");

// Changing algorithm is a rehash too.
const bcrypt = await password.hash("pw", {{ algorithm: "bcrypt", cost: 4 }});
eq(password.needsRehash(bcrypt, {{ algorithm: "bcrypt", cost: 4 }}), false, "bcrypt at cost");
eq(password.needsRehash(bcrypt, {{ algorithm: "bcrypt" }}), true, "bcrypt below default cost");
eq(password.needsRehash(bcrypt), true, "bcrypt is not argon2id");
eq(password.needsRehash(weak, {{ algorithm: "bcrypt" }}), true, "argon2id is not bcrypt");

const scrypt = await password.hash("pw", {{ algorithm: "scrypt", cost: 10 }});
eq(password.needsRehash(scrypt, {{ algorithm: "scrypt", cost: 10 }}), false, "scrypt at cost");
eq(password.needsRehash(scrypt, {{ algorithm: "scrypt" }}), true, "scrypt below default cost");

// The whole login flow, as it would be written.
const stored = weak;
let replaced = null;
if (await password.verify("pw", stored) && password.needsRehash(stored)) {{
  replaced = await password.hash("pw", {{ memoryCost: 128, timeCost: 1 }});
}}
eq(await password.verify("pw", replaced), true, "the replacement verifies");
eq(password.needsRehash(replaced, {{ memoryCost: 128, timeCost: 1 }}), false, "and is current");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// Every refusal a caller can walk into, so none of them is a silent wrong
/// answer. bcrypt's 72-byte budget is the one that matters most: truncating
/// would quietly make two different passwords the same password.
#[test]
fn password_refusals_are_explicit() {
    let s = run(
        "hash_password_errors.mjs",
        &format!(
            r#"
import {{ password }} from "runtime:hashing";
{EQ}
const rejects = async (fn, what) => {{
  try {{ await fn(); }} catch (e) {{ return e; }}
  throw new Error(`${{what}}: expected a refusal`);
}};

eq((await rejects(() => password.hash("pw", {{ algorithm: "md5" }}), "md5")) instanceof TypeError, true, "unknown algorithm");
eq((await rejects(() => password.verify("pw", "not a hash"), "garbage")) instanceof TypeError, true, "unreadable hash");
eq((await rejects(() => password.verify("pw", 42), "number")) instanceof TypeError, true, "a number is not a hash");
eq((await rejects(() => password.verify("pw", "$pbkdf2-sha256$i=1$c2FsdA$aGFzaA"), "pbkdf2")) instanceof TypeError, true, "unsupported algorithm");

// bcrypt hashes at most 72 bytes including the NUL it appends, so 71 is the
// last length that is hashed whole — past it we refuse rather than truncate.
const ok = await password.hash("a".repeat(71), {{ algorithm: "bcrypt", cost: 4 }});
eq(await password.verify("a".repeat(71), ok), true, "71 bytes hashes");
eq((await rejects(() => password.hash("a".repeat(72), {{ algorithm: "bcrypt", cost: 4 }}), "72")) instanceof TypeError, true, "72 bytes refused");

// Argon2 and scrypt have no such limit: a long passphrase is hashed whole.
const long = "a".repeat(200);
eq(await password.verify(long, await password.hash(long, {{ memoryCost: 64, timeCost: 1 }})), true, "argon2 long");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The claim the module is built on: hashing is pure computation, so none of it
/// needs authority. Under `--deny-all` the import must resolve (D26/D38) and
/// every function except the one that draws a salt must work.
#[test]
fn hashing_needs_no_capability() {
    let s = run_denied(
        "hash_denied.mjs",
        &format!(
            r#"
import {{ hash, Hasher, hashStream, hmac, timingSafeEqual, password }} from "runtime:hashing";
{EQ}
eq(hash("sha256", "abc", "hex"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", "hash");
eq(new Hasher("sha256").update("abc").digest("hex"), hash("sha256", "abc", "hex"), "Hasher");
eq(hmac("sha256", "k", "d", "hex").length, 64, "hmac");
eq(timingSafeEqual("a", "a"), true, "timingSafeEqual");

const stream = new Response("payload").body;
eq(await hashStream("sha256", stream, "hex"), hash("sha256", "payload", "hex"), "hashStream");

// Verifying needs no randomness — the salt is inside the stored string — so a
// service that only checks passwords needs nothing granted.
const stored = "$argon2id$v=19$m=64,t=1,p=1$9t9Wzd5MS0lI3+YX/SK/HQ$CyaOes7VbjwtOU3aOk9hIIcta6m3GmWyJddALH5P2QQ";
eq(await password.verify("correct horse", stored), true, "verify");
eq(await password.verify("wrong", stored), false, "verify rejects");
eq(password.needsRehash(stored), true, "needsRehash");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// The one thing that is not free: a fresh salt is randomness, and randomness
/// is the Entropy provider's to give. Supplying the salt takes even that away.
#[test]
fn a_supplied_salt_is_reproducible_and_needs_nothing() {
    let s = run_denied(
        "hash_salt.mjs",
        &format!(
            r#"
import {{ password }} from "runtime:hashing";
{EQ}
const salt = new Uint8Array(16).fill(7);
const options = {{ memoryCost: 64, timeCost: 1, salt }};
const a = await password.hash("pw", options);
const b = await password.hash("pw", options);
eq(a, b, "the same salt gives the same hash");
eq(await password.verify("pw", a), true, "and it verifies");

// A different salt, the same password: a different hash. This is the property
// the salt exists for.
const c = await password.hash("pw", {{ ...options, salt: new Uint8Array(16).fill(9) }});
if (c === a) throw new Error("the salt made no difference");

// bcrypt's salt field is exactly 16 bytes, so a wrong length is refused rather
// than padded into something that cannot be read back.
let threw = false;
try {{
  await password.hash("pw", {{ algorithm: "bcrypt", cost: 4, salt: new Uint8Array(8) }});
}} catch (e) {{ threw = e instanceof TypeError; }}
eq(threw, true, "a short bcrypt salt is refused");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// `runtime:hashing` and `crypto.subtle` must not disagree about a digest —
/// they are the same algorithms, and one of them is the standard.
#[test]
fn the_shared_algorithms_agree_with_crypto_subtle() {
    let s = run(
        "hash_vs_subtle.mjs",
        &format!(
            r#"
import {{ hash, timingSafeEqual }} from "runtime:hashing";
{EQ}
const data = new TextEncoder().encode("the quick brown fox");
for (const [webcrypto, ours] of [["SHA-1", "sha1"], ["SHA-256", "sha256"], ["SHA-384", "sha384"], ["SHA-512", "sha512"]]) {{
  const viaSubtle = new Uint8Array(await crypto.subtle.digest(webcrypto, data));
  eq(timingSafeEqual(viaSubtle, hash(ours, data)), true, `${{ours}} agrees with ${{webcrypto}}`);
}}
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}

/// A default import, for the module's other supported shape.
#[test]
fn the_default_export_carries_the_same_surface() {
    let s = run(
        "hash_default.mjs",
        &format!(
            r#"
import hashing from "runtime:hashing";
{EQ}
for (const name of ["hash", "Hasher", "hashStream", "hmac", "timingSafeEqual", "password"]) {{
  if (!hashing[name]) throw new Error(`missing ${{name}}`);
}}
eq(hashing.hash("sha256", "abc", "hex").slice(0, 8), "ba7816bf", "hash");
eq(new hashing.Hasher("md5").update("abc").digest("hex"), "900150983cd24fb0d6963f7d28e17f72", "Hasher");
console.log("ok");
"#
        ),
    );
    assert!(s.contains("ok"), "{s}");
}
