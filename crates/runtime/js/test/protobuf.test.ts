import { expect, test } from "bun:test";
import { Schema } from "../serialization/protobuf/schema.js";

test("exact wire bytes for a simple message", () => {
  const s = new Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const bytes = s.encode("M", { a: 150, b: "hi" });
  expect([...bytes]).toEqual([0x08, 0x96, 0x01, 0x12, 0x02, 0x68, 0x69]);
  expect(s.decode("M", bytes)).toEqual({ a: 150, b: "hi" });
});

test("all scalar types round-trip (proto3)", () => {
  const s = new Schema(`
    syntax = "proto3";
    package t;
    enum Color { RED = 0; GREEN = 1; BLUE = 2; }
    message Inner { string v = 1; }
    message All {
      int32 i32 = 1; int64 i64 = 2; uint32 u32 = 3; uint64 u64 = 4;
      sint32 s32 = 5; sint64 s64 = 6; fixed32 f32 = 7; fixed64 f64 = 8;
      sfixed32 sf32 = 9; sfixed64 sf64 = 10; float fl = 11; double db = 12;
      bool b = 13; string s = 14; bytes by = 15; Color c = 16;
      Inner inner = 17;
    }
  `);
  const input = {
    i32: -7, i64: 9007199254740993n, u32: 7, u64: 18446744073709551615n,
    s32: -123, s64: -9007199254740993n, f32: 4294967295, f64: 18446744073709551615n,
    sf32: -42, sf64: -99n, fl: 1.5, db: 44.95,
    b: true, s: 'héllo 𐍈 "q"', by: new Uint8Array([1, 2, 3, 255]), c: "BLUE",
    inner: { v: "x" },
  };
  expect(s.decode("t.All", s.encode("t.All", input))).toEqual(input);
});

test("repeated packed + expanded, maps, oneof", () => {
  const s = new Schema(`
    syntax = "proto3";
    message M {
      repeated int32 nums = 1;                 // packed by default in proto3
      repeated string tags = 2;                // length-delimited (never packed)
      map<string, int32> counts = 3;
      map<int32, string> names = 4;
      oneof choice { int32 a = 5; string b = 6; }
    }
  `);
  const input = {
    nums: [1, 2, 300, -4],
    tags: ["x", "y"],
    counts: { x: 1, y: 2 },
    names: { "1": "one", "2": "two" },
    b: "picked",
  };
  expect(s.decode("M", s.encode("M", input))).toEqual(input);
});

test("oneof last-wins on decode", () => {
  const s = new Schema(`syntax="proto3"; message M { oneof k { int32 a = 1; int32 b = 2; } }`);
  // hand-build a buffer with both a and b set; b (field 2) appears last.
  const both = new Uint8Array([0x08, 0x05, 0x10, 0x07]); // a=5, b=7
  expect(s.decode("M", both)).toEqual({ b: 7 });
});

test("implicit presence omits defaults; explicit (optional) keeps them", () => {
  const s = new Schema(`syntax="proto3"; message M { int32 a = 1; optional int32 b = 2; }`);
  // a=0 (implicit) omitted; b=0 (explicit) written.
  const bytes = s.encode("M", { a: 0, b: 0 });
  expect([...bytes]).toEqual([0x10, 0x00]); // only field 2
  expect(s.decode("M", bytes)).toEqual({ b: 0 });
});

test("edition 2023 gives explicit presence by default", () => {
  const s = new Schema(`edition = "2023"; message M { int32 a = 1; }`);
  // a=0 with explicit presence (2023 default) is written.
  const bytes = s.encode("M", { a: 0 });
  expect([...bytes]).toEqual([0x08, 0x00]);
  expect(s.decode("M", bytes)).toEqual({ a: 0 });
});

test("edition 2024 parses and shares 2023 wire defaults (explicit presence)", () => {
  const s = new Schema(`edition = "2024"; message M { int32 a = 1; }`);
  // a=0 with explicit presence (editions default) is written, same as 2023.
  const bytes = s.encode("M", { a: 0 });
  expect([...bytes]).toEqual([0x08, 0x00]);
  expect(s.decode("M", bytes)).toEqual({ a: 0 });
});

test("unsupported editions are rejected with a clear error", () => {
  expect(() => new Schema(`edition = "2025"; message M { int32 a = 1; }`)).toThrow(/unsupported edition "2025"/);
});

test("unknown fields are preserved across re-encode", () => {
  const full = new Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const original = full.encode("M", { a: 5, b: "keep" });
  const partial = new Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  const decoded = partial.decode("M", original);
  expect(decoded.a).toBe(5);
  // re-encoding the partial decode must still carry field b
  const reencoded = partial.encode("M", decoded);
  expect(full.decode("M", reencoded)).toEqual({ a: 5, b: "keep" });
});

// --- regressions found by the official protobuf conformance suite ---

test("negative enum values parse", () => {
  const s = new Schema(`syntax="proto3"; enum E { Z = 0; NEG = -1; } message M { E e = 1; }`);
  expect(s.decode("M", s.encode("M", { e: "NEG" }))).toEqual({ e: "NEG" });
});

test("editions delimited (group) message encoding round-trips", () => {
  const s = new Schema(`edition = "2023";
    option features.message_encoding = DELIMITED;
    message M { Sub sub = 1; repeated Sub many = 2; }
    message Sub { int32 a = 1; }
  `);
  const input = { sub: { a: 99 }, many: [{ a: 1 }, { a: 2 }] };
  const bytes = s.encode("M", input);
  // group: start-group tag (1<<3|3=0x0B), a=99 (0x08 0x63), end-group (1<<3|4=0x0C).
  expect([...bytes.slice(0, 4)]).toEqual([0x0b, 0x08, 0x63, 0x0c]);
  expect(s.decode("M", bytes)).toEqual(input);
});

test("singular message fields merge across repeated occurrences", () => {
  const s = new Schema(`syntax="proto3"; message Inner { int32 a = 1; int32 b = 2; } message M { Inner inner = 1; }`);
  const a = s.encode("M", { inner: { a: 1 } });
  const b = s.encode("M", { inner: { b: 2 } });
  const concat = new Uint8Array([...a, ...b]);
  expect(s.decode("M", concat)).toEqual({ inner: { a: 1, b: 2 } });
});

test("unknown length-delimited field is skipped without desync", () => {
  const full = new Schema(`syntax="proto3"; message M { string a = 1; int32 b = 2; }`);
  const partial = new Schema(`syntax="proto3"; message M { int32 b = 2; }`);
  // field a (unknown to partial) precedes known field b — a skip off-by-one would corrupt b.
  const decoded = partial.decode("M", full.encode("M", { a: "skipme", b: 99 }));
  expect(decoded.b).toBe(99);
});

test("rejects truncated, overlong, and field-0 inputs", () => {
  const s = new Schema(`syntax="proto3"; message M { int64 n = 1; }`);
  const valid = s.encode("M", { n: 300n });
  expect(() => s.decode("M", valid.subarray(0, valid.length - 1))).toThrow(); // truncated varint
  expect(() => s.decode("M", new Uint8Array([0x08, ...Array(11).fill(0x80)]))).toThrow(/too long/); // overlong
  expect(() => s.decode("M", new Uint8Array([0x00, 0x01]))).toThrow(/field number 0/); // tag field 0
});

test("rejects proto2", () => {
  expect(() => new Schema(`syntax="proto2"; message M {}`)).toThrow(/proto2/);
  expect(() => new Schema(`syntax="proto3"; message M { required int32 a = 1; }`)).toThrow(/proto2|required/);
});
