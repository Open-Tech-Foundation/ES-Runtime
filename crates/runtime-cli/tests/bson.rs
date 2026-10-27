//! End-to-end tests for `BSON` in `runtime:serialization` (DECISIONS D187).
//!
//! The byte format itself is checked against MongoDB's corpus by
//! `mongodb-specs/bson-corpus.js`; these cover what the corpus cannot: the
//! JavaScript side of the mapping, the classes, the errors, and the prose tests
//! the corpus specification asks every driver to write by hand.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn write(name: &str, contents: &str) -> PathBuf {
    let path = common::tmpdir().join(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn esrun() -> Command {
    let mut command = Command::new(common::esrun_bin());
    command.current_dir(common::tmpdir());
    command
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `source` with no grants at all and returns its stdout lines: BSON reads
/// and reaches nothing, so it needs no capability.
fn run(name: &str, source: &str) -> Vec<String> {
    let script = format!("import {{ BSON }} from \"runtime:serialization\";\n{source}");
    let app = write(name, &script);
    let out = esrun().arg(&app).output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn each_javascript_value_reads_back_as_its_documented_type() {
    let out = run(
        "bson_types.mjs",
        r#"
const { ObjectId, Decimal128, UUID, Timestamp, Binary, Code, BSONRegExp, MinKey, MaxKey, Double } = BSON;
const back = BSON.decode(BSON.encode({
  s: "héllo", i: 42, d: 1.5, neg0: -0, whole: 2 ** 40, big: 2n ** 60n, t: true, n: null, gone: undefined,
  date: new Date(1356351330501), re: /a\/b/gim, bytes: new Uint8Array([1, 2]), ab: new Uint8Array([3]).buffer,
  arr: [1, undefined, "x"], map: new Map([["k", 1]]), oid: new ObjectId("57e193d7a9cc81b4027498b5"),
  dec: new Decimal128("1.50"), uuid: new UUID("73ffd264-44b3-4c69-90e8-e7d1dfc035d4"),
  ts: new Timestamp({ t: 42, i: 1 }), bin: new Binary(new Uint8Array([9]), 0x80),
  code: new Code("f()"), scoped: new Code("g()", { y: 1 }), bre: new BSONRegExp("^a", "xi"),
  min: new MinKey(), max: new MaxKey(), dbl: new Double(3),
}));
const show = (v) => v === null ? "null" : v instanceof Uint8Array ? `Uint8Array[${v}]` : typeof v === "object" ? `${v.constructor.name}` : `${typeof v} ${String(v)}`;
for (const [k, v] of Object.entries(back)) console.log(k, show(v));
console.log("gone" in back, Object.is(back.neg0, -0), back.arr[1]);
console.log(back.oid.toHexString(), back.dec.toString(), back.uuid.toString(), back.ts.t, back.ts.i);
console.log(back.bin.subType, back.scoped.scope.y, back.code.scope, back.bre.pattern, back.bre.options);
console.log(back.re.pattern, back.re.options, back.date.toISOString(), back.map.k);
"#,
    );
    assert_eq!(
        out,
        [
            "s string héllo",
            "i number 42",
            "d number 1.5",
            "neg0 number 0",
            "whole number 1099511627776",
            "big bigint 1152921504606846976",
            "t boolean true",
            "n null",
            "date Date",
            "re BSONRegExp",
            "bytes Uint8Array[1,2]",
            "ab Uint8Array[3]",
            "arr Array",
            "map Object",
            "oid ObjectId",
            "dec Decimal128",
            "uuid UUID",
            "ts Timestamp",
            "bin Binary",
            "code Code",
            "scoped Code",
            "bre BSONRegExp",
            "min MinKey",
            "max MaxKey",
            "dbl number 3",
            "false true null",
            "57e193d7a9cc81b4027498b5 1.50 73ffd264-44b3-4c69-90e8-e7d1dfc035d4 42 1",
            "128 1 null ^a ix",
            "a\\/b im 2012-12-24T12:15:30.501Z 1",
        ]
    );
}

#[test]
fn numbers_take_int32_or_double_and_bigint_takes_int64() {
    // Byte 4 of `{ v: … }` is the element type.
    let out = run(
        "bson_numbers.mjs",
        r#"
const type = (v) => BSON.encode({ v })[4].toString(16).padStart(2, "0");
for (const v of [1, -1, 2 ** 31 - 1, -(2 ** 31), 2 ** 31, 1.5, -0, NaN, 1n, new BSON.Double(1)]) {
  console.log(String(v instanceof BSON.Double ? "Double(1)" : typeof v === "bigint" ? v + "n" : Object.is(v, -0) ? "-0" : v), type(v));
}
// An int64 always reads back as a bigint, however small.
console.log(typeof BSON.decode(BSON.encode({ v: 5n })).v);
"#,
    );
    assert_eq!(
        out,
        [
            "1 10",
            "-1 10",
            "2147483647 10",
            "-2147483648 10",
            "2147483648 01",
            "1.5 01",
            "-0 01",
            "NaN 01",
            "1n 12",
            "Double(1) 01",
            "bigint",
        ]
    );
}

/// The corpus specification's prose test 1: NUL is refused in a field name at
/// any depth and in either part of a regular expression.
#[test]
fn a_nul_is_refused_in_names_and_regular_expressions() {
    let out = run(
        "bson_nul.mjs",
        r#"
for (const make of [
  () => BSON.encode({ "a\0b": 1 }),
  () => BSON.encode({ sub: { "a\0b": 1 } }),
  () => BSON.encode({ m: new Map([["a\0", 1]]) }),
  () => new BSON.BSONRegExp("a\0", "i"),
  () => BSON.encode({ r: new RegExp("a\0") }),
]) {
  try { make(); console.log("allowed"); } catch (e) { console.log(e.name); }
}
// A NUL inside a string value is fine: strings are length-prefixed.
console.log(BSON.decode(BSON.encode({ s: "a\0b" })).s.length);
"#,
    );
    assert_eq!(
        out,
        [
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "3"
        ]
    );
}

/// The corpus specification's prose test 2: a document with a repeated key
/// decodes with the last value winning, and re-encodes with one.
#[test]
fn a_duplicate_key_keeps_its_last_value() {
    let out = run(
        "bson_dup.mjs",
        r#"
const data = Uint8Array.from("1700000010666f6f000100000010666f6f000200000000".match(/../g), (h) => parseInt(h, 16));
const doc = BSON.decode(data);
console.log(JSON.stringify(doc));
const again = BSON.encode(doc);
console.log(again.length, JSON.stringify(BSON.decode(again)));
"#,
    );
    assert_eq!(out, ["{\"foo\":2}", "14 {\"foo\":2}"]);
}

#[test]
fn a_decoded_document_has_ordinary_prototypes_and_own_keys() {
    let out = run(
        "bson_proto.mjs",
        r#"
const doc = BSON.decode(BSON.encode(JSON.parse('{"__proto__": {"polluted": true}, "constructor": 1, "2": "b", "1": "a"}')));
console.log(Object.getPrototypeOf(doc) === Object.prototype, Object.hasOwn(doc, "__proto__"), ({}).polluted);
console.log(Object.keys(doc).join(","));
"#,
    );
    // Integer-like keys come first, as in any JavaScript object.
    assert_eq!(out, ["true true undefined", "1,2,__proto__,constructor"]);
}

#[test]
fn what_cannot_be_encoded_says_so() {
    let out = run(
        "bson_refuse.mjs",
        r#"
const cyclic = {}; cyclic.self = cyclic;
const cyclicArray = []; cyclicArray.push(cyclicArray);
for (const [label, value] of [
  ["array", [1]], ["null", null], ["string", "x"], ["ObjectId", new BSON.ObjectId()],
  ["cycle", cyclic], ["array cycle", { a: cyclicArray }], ["function", { f() {} }], ["symbol", { s: Symbol() }],
  ["int64 range", { b: 2n ** 63n }], ["invalid date", { d: new Date(NaN) }], ["Float32Array", { v: new Float32Array(1) }],
  ["Set", { s: new Set() }], ["map key", { m: new Map([[1, 1]]) }], ["subType", { b: Object.assign(new BSON.Binary(), { subType: 256 }) }],
]) {
  try { BSON.encode(value); console.log(label, "allowed"); } catch (e) { console.log(label, e.name); }
}
"#,
    );
    assert_eq!(
        out,
        [
            "array TypeError",
            "null TypeError",
            "string TypeError",
            "ObjectId TypeError",
            "cycle TypeError",
            "array cycle TypeError",
            "function TypeError",
            "symbol TypeError",
            "int64 range RangeError",
            "invalid date RangeError",
            "Float32Array TypeError",
            "Set TypeError",
            "map key TypeError",
            "subType RangeError",
        ]
    );
}

#[test]
fn malformed_bytes_are_a_syntax_error_and_validate_agrees() {
    let out = run(
        "bson_bad.mjs",
        r#"
const hex = (h) => Uint8Array.from(h.match(/../g), (x) => parseInt(x, 16));
for (const h of ["0500000001", "0600000000", "050000000000", "0900000008620002 00".replace(" ", ""), "0C0000000261000100000000", "0D000000106100010000"]) {
  const bytes = hex(h);
  let name = "decoded";
  try { BSON.decode(bytes); } catch (e) { name = e.name; }
  console.log(name, BSON.validate(bytes), BSON.validate(bytes, { detailed: true }).valid);
}
console.log(BSON.validate(BSON.encode({ a: 1 })));
try { BSON.decode("not bytes"); } catch (e) { console.log(e.name); }
"#,
    );
    assert_eq!(
        out,
        [
            "SyntaxError false false",
            "SyntaxError false false",
            "SyntaxError false false",
            "SyntaxError false false",
            "SyntaxError false false",
            "SyntaxError false false",
            "true",
            "TypeError",
        ]
    );
}

#[test]
fn nesting_is_bounded_both_ways() {
    let out = run(
        "bson_depth.mjs",
        r#"
let deep = {};
for (let i = 0; i < 300; i++) deep = { d: deep };
try { BSON.encode(deep); } catch (e) { console.log(e.name); }
let ok = {};
for (let i = 0; i < 200; i++) ok = { d: ok };
console.log(BSON.encode(ok).length > 0);
"#,
    );
    assert_eq!(out, ["RangeError", "true"]);
}

#[test]
fn object_ids_are_unique_ordered_and_carry_their_time() {
    let out = run(
        "bson_oid.mjs",
        r#"
const { ObjectId } = BSON;
const ids = Array.from({ length: 1000 }, () => new ObjectId());
console.log(new Set(ids.map(String)).size);
// Same process, same second: the counter orders them.
console.log(ids.every((id, i) => i === 0 || id.toHexString().slice(8, 18) === ids[0].toHexString().slice(8, 18)));
console.log(Math.abs(ids[0].getTimestamp().getTime() - Date.now()) < 5000);
const a = new ObjectId("57E193D7A9CC81B4027498B5");
console.log(a.toHexString(), a.equals("57e193d7a9cc81b4027498b5"), a.equals(new ObjectId(a.id)), JSON.stringify({ a }));
console.log(ObjectId.isValid("zz"), ObjectId.isValid(a.toHexString()), ObjectId.createFromTime(1).toHexString());
try { new ObjectId("123"); } catch (e) { console.log(e.name); }
"#,
    );
    assert_eq!(
        out,
        [
            "1000",
            "true",
            "true",
            "57e193d7a9cc81b4027498b5 true true {\"a\":\"57e193d7a9cc81b4027498b5\"}",
            "false true 000000010000000000000000",
            "TypeError",
        ]
    );
}

#[test]
fn uuid_timestamp_and_decimal_follow_their_rules() {
    let out = run(
        "bson_misc.mjs",
        r#"
const { UUID, Timestamp, Decimal128 } = BSON;
const u = new UUID();
console.log(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(u.toString()), u.equals(new UUID(u.bytes)));
console.log(new UUID("73FFD26444B34C6990E8E7D1DFC035D4").toHexString(), new UUID("73ffd264-44b3-4c69-90e8-e7d1dfc035d4").toHexString(false));
console.log(new Timestamp(0x0000002a00000001n).t, new Timestamp({ t: 1, i: 2 }).i, JSON.stringify(new Timestamp({ t: 1, i: 2 })));
for (const bad of [{ t: -1, i: 0 }, { t: 2 ** 32, i: 0 }, { t: 1.5, i: 0 }, -1n]) {
  try { new Timestamp(bad); console.log("allowed"); } catch (e) { console.log(e.name); }
}
console.log(new Decimal128("1.0E+3").toString(), new Decimal128("-0").toString(), JSON.stringify(new Decimal128("0.1")));
for (const bad of ["1.2.3", "1E+7000", "12345678901234567890123456789012345", ""]) {
  try { new Decimal128(bad); console.log("allowed"); } catch (e) { console.log(e.name); }
}
"#,
    );
    assert_eq!(
        out,
        [
            "true true",
            "73ffd264-44b3-4c69-90e8-e7d1dfc035d4 73ffd26444b34c6990e8e7d1dfc035d4",
            "42 2 {\"$timestamp\":{\"t\":1,\"i\":2}}",
            "RangeError",
            "RangeError",
            "RangeError",
            "RangeError",
            "1.0E+3 -0 {\"$numberDecimal\":\"0.1\"}",
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
            "SyntaxError",
        ]
    );
}

#[test]
fn vectors_round_trip_and_refuse_bad_padding() {
    let out = run(
        "bson_vector.mjs",
        r#"
const { Binary } = BSON;
const read = (b) => BSON.decode(BSON.encode({ v: b })).v;
console.log([...read(Binary.fromFloat32Array([1.5, -2])).toFloat32Array()].join(","));
console.log([...read(Binary.fromInt8Array([-128, 127])).toInt8Array()].join(","));
console.log([...read(Binary.fromPackedBits([0b11110000], 4)).toPackedBits()].join(","), read(Binary.fromPackedBits([1])).buffer[1]);
// The ignored bits of a packed vector must be zero, writing and reading.
for (const make of [
  () => Binary.fromPackedBits([0b11111111], 7),
  () => Binary.fromPackedBits([], 1),
  () => Binary.fromInt8Array([128]),
  () => new Binary(Uint8Array.of(0x10, 7, 0xff), 9).toPackedBits(),
  () => Binary.fromInt8Array([1]).toFloat32Array(),
  () => new Binary(Uint8Array.of(1), 0).toInt8Array(),
]) {
  try { make(); console.log("allowed"); } catch (e) { console.log(e.name); }
}
"#,
    );
    assert_eq!(
        out,
        [
            "1.5,-2",
            "-128,127",
            "240 0",
            "RangeError",
            "RangeError",
            "RangeError",
            "RangeError",
            "TypeError",
            "TypeError",
        ]
    );
}

#[test]
fn a_bson_regexp_becomes_a_regexp_only_when_javascript_can_say_it() {
    let out = run(
        "bson_regexp.mjs",
        r#"
const { BSONRegExp } = BSON;
console.log(String(new BSONRegExp("^a", "mi").toRegExp()));
try { new BSONRegExp("a", "x").toRegExp(); } catch (e) { console.log(e.name); }
try { new BSONRegExp("a", "q"); } catch (e) { console.log(e.name); }
"#,
    );
    assert_eq!(out, ["/^a/im", "SyntaxError", "SyntaxError"]);
}
