import { expect, test } from "bun:test";
import { Schema } from "../serialization/protobuf/schema.js";

test("exact wire bytes for a simple message", () => {
  const s = new Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const bytes = s.build("M", { a: 150, b: "hi" });
  expect([...bytes]).toEqual([0x08, 0x96, 0x01, 0x12, 0x02, 0x68, 0x69]);
  expect(s.parse("M", bytes)).toEqual({ a: 150, b: "hi" });
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
  expect(s.parse("t.All", s.build("t.All", input))).toEqual(input);
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
  expect(s.parse("M", s.build("M", input))).toEqual(input);
});

test("oneof last-wins on decode", () => {
  const s = new Schema(`syntax="proto3"; message M { oneof k { int32 a = 1; int32 b = 2; } }`);
  // hand-build a buffer with both a and b set; b (field 2) appears last.
  const both = new Uint8Array([0x08, 0x05, 0x10, 0x07]); // a=5, b=7
  expect(s.parse("M", both)).toEqual({ b: 7 });
});

test("implicit presence omits defaults; explicit (optional) keeps them", () => {
  const s = new Schema(`syntax="proto3"; message M { int32 a = 1; optional int32 b = 2; }`);
  // a=0 (implicit) omitted; b=0 (explicit) written.
  const bytes = s.build("M", { a: 0, b: 0 });
  expect([...bytes]).toEqual([0x10, 0x00]); // only field 2
  expect(s.parse("M", bytes)).toEqual({ b: 0 });
});

test("edition 2023 gives explicit presence by default", () => {
  const s = new Schema(`edition = "2023"; message M { int32 a = 1; }`);
  // a=0 with explicit presence (2023 default) is written.
  const bytes = s.build("M", { a: 0 });
  expect([...bytes]).toEqual([0x08, 0x00]);
  expect(s.parse("M", bytes)).toEqual({ a: 0 });
});

test("unknown fields are preserved across re-encode", () => {
  const full = new Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const original = full.build("M", { a: 5, b: "keep" });
  const partial = new Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  const decoded = partial.parse("M", original);
  expect(decoded.a).toBe(5);
  // re-encoding the partial decode must still carry field b
  const reencoded = partial.build("M", decoded);
  expect(full.parse("M", reencoded)).toEqual({ a: 5, b: "keep" });
});

test("rejects proto2", () => {
  expect(() => new Schema(`syntax="proto2"; message M {}`)).toThrow(/proto2/);
  expect(() => new Schema(`syntax="proto3"; message M { required int32 a = 1; }`)).toThrow(/proto2|required/);
});
