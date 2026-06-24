import { expect, test } from "bun:test";
import { Schema } from "../serialization/protobuf/schema.js";

test("scalar JSON mapping: 64-bit as string, bytes as base64, enum as name", () => {
  const s = new Schema(`
    syntax = "proto3";
    enum Color { RED = 0; GREEN = 1; }
    message M {
      int32 i = 1; int64 big = 2; uint64 ubig = 3;
      double d = 4; bool b = 5; string s = 6; bytes by = 7; Color c = 8;
    }
  `);
  const value = {
    i: -7, big: 9007199254740993n, ubig: 18446744073709551615n,
    d: 1.5, b: true, s: "hi", by: new Uint8Array([1, 2, 3, 255]), c: "GREEN",
  };
  const json = s.toJson("M", value);
  expect(json).toEqual({
    i: -7, big: "9007199254740993", ubig: "18446744073709551615",
    d: 1.5, b: true, s: "hi", by: "AQID/w==", c: "GREEN",
  });
  expect(s.fromJson("M", json)).toEqual(value);
});

test("JSON accepts field name and number for enums; round-trips repeated/map", () => {
  const s = new Schema(`
    syntax = "proto3";
    enum E { Z = 0; A = 1; }
    message M {
      repeated int64 nums = 1;
      map<string, int32> counts = 2;
      E e = 3;
    }
  `);
  const value = { nums: [1n, 2n], counts: { x: 1 }, e: "A" };
  expect(s.toJson("M", value)).toEqual({ nums: ["1", "2"], counts: { x: 1 }, e: "A" });
  // numeric enum on input is accepted
  expect(s.fromJson("M", { nums: ["1", "2"], counts: { x: 1 }, e: 1 })).toEqual(value);
});

test("non-finite floats map to strings", () => {
  const s = new Schema(`syntax="proto3"; message M { double d = 1; float f = 2; }`);
  expect(s.toJson("M", { d: Infinity, f: NaN })).toEqual({ d: "Infinity", f: "NaN" });
  expect(s.fromJson("M", { d: "-Infinity", f: "NaN" })).toEqual({ d: -Infinity, f: NaN });
});

const WKT_IMPORTS = `
  import "google/protobuf/timestamp.proto";
  import "google/protobuf/duration.proto";
  import "google/protobuf/wrappers.proto";
  import "google/protobuf/struct.proto";
  import "google/protobuf/field_mask.proto";
  import "google/protobuf/any.proto";
`;

test("Timestamp and Duration map to strings", () => {
  const s = new Schema({
    "m.proto": `syntax="proto3"; ${WKT_IMPORTS}
      message M {
        google.protobuf.Timestamp ts = 1;
        google.protobuf.Duration dur = 2;
      }`,
  });
  const value = {
    ts: { seconds: 63075600n, nanos: 21000000 }, // 1972-01-01T01:00:00.021Z (UTC)
    dur: { seconds: 3n, nanos: 1 },
  };
  const json = s.toJson("M", value) as Record<string, string>;
  expect(json.ts).toBe("1972-01-01T01:00:00.021Z");
  expect(json.dur).toBe("3.000000001s");
  expect(s.fromJson("M", json)).toEqual(value);
});

test("wrappers map to bare values", () => {
  const s = new Schema({
    "m.proto": `syntax="proto3"; ${WKT_IMPORTS}
      message M {
        google.protobuf.Int64Value n = 1;
        google.protobuf.StringValue s = 2;
        google.protobuf.BytesValue b = 3;
      }`,
  });
  const value = { n: { value: 42n }, s: { value: "hi" }, b: { value: new Uint8Array([1, 2]) } };
  expect(s.toJson("M", value)).toEqual({ n: "42", s: "hi", b: "AQI=" });
  expect(s.fromJson("M", { n: "42", s: "hi", b: "AQI=" })).toEqual(value);
});

test("Struct / Value / ListValue map to native JSON", () => {
  const s = new Schema({
    "m.proto": `syntax="proto3"; ${WKT_IMPORTS}
      message M { google.protobuf.Struct data = 1; }`,
  });
  const native = { a: 1, b: "x", c: true, d: null, e: [1, "y"], f: { g: 2 } };
  const value = s.fromJson("M", { data: native });
  expect(s.toJson("M", value)).toEqual({ data: native });
});

test("FieldMask maps to a comma-joined camelCase string", () => {
  const s = new Schema({
    "m.proto": `syntax="proto3"; ${WKT_IMPORTS}
      message M { google.protobuf.FieldMask mask = 1; }`,
  });
  const json = { mask: "user.displayName,user.email" };
  const value = s.fromJson("M", json);
  expect(value).toEqual({ mask: { paths: ["user.display_name", "user.email"] } });
  expect(s.toJson("M", value)).toEqual(json);
});

test("Any embeds @type and round-trips, including a WKT payload", () => {
  const s = new Schema({
    "m.proto": `syntax="proto3"; ${WKT_IMPORTS}
      message Inner { int32 a = 1; string b = 2; }
      message M { google.protobuf.Any any = 1; }`,
  });
  // message payload: spread alongside @type
  const j1 = { any: { "@type": "type.googleapis.com/Inner", a: 5, b: "x" } };
  expect(s.toJson("M", s.fromJson("M", j1))).toEqual(j1);

  // WKT payload with non-object JSON: nested under "value"
  const j2 = { any: { "@type": "type.googleapis.com/google.protobuf.Duration", value: "2.500s" } };
  expect(s.toJson("M", s.fromJson("M", j2))).toEqual(j2);
});
