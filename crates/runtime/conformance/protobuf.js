// Conformance for the pure-JS Protobuf implementation in runtime:serialization.
// Each test() is one tallied assertion. BigInt-aware deep equality (JSON.stringify
// can't serialize BigInt) compares values structurally.
function deepEq(a, b, msg) {
  function norm(o) {
    if (typeof o === "bigint") return "b:" + o.toString();
    if (o instanceof Uint8Array) return "u:" + Array.from(o).join(",");
    if (Array.isArray(o)) return o.map(norm);
    if (o && typeof o === "object") {
      return Object.keys(o).sort().reduce((acc, k) => ((acc[k] = norm(o[k])), acc), {});
    }
    return o;
  }
  const x = JSON.stringify(norm(a));
  const y = JSON.stringify(norm(b));
  if (x !== y) throw new Error(`${msg}: expected ${y}, got ${x}`);
}

test("protobuf: exact wire bytes + round-trip", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const bytes = s.encode("M", { a: 150, b: "hi" });
  deepEq(Array.from(bytes), [0x08, 0x96, 0x01, 0x12, 0x02, 0x68, 0x69], "wire bytes");
  deepEq(s.decode("M", bytes), { a: 150, b: "hi" }, "round-trip");
});

test("protobuf: all scalar types incl 64-bit BigInt round-trip", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema(`
    syntax = "proto3"; package t;
    enum Color { RED = 0; BLUE = 2; }
    message Inner { string v = 1; }
    message All {
      int32 i32 = 1; int64 i64 = 2; uint64 u64 = 4; sint64 s64 = 6;
      fixed64 f64 = 8; sfixed32 sf32 = 9; float fl = 11; double db = 12;
      bool b = 13; string s = 14; bytes by = 15; Color c = 16; Inner inner = 17;
    }
  `);
  const input = {
    i32: -7, i64: 9007199254740993n, u64: 18446744073709551615n, s64: -9007199254740993n,
    f64: 18446744073709551615n, sf32: -42, fl: 1.5, db: 44.95,
    b: true, s: 'héllo 𐍈', by: new Uint8Array([1, 2, 3, 255]), c: "BLUE", inner: { v: "x" },
  };
  deepEq(s.decode("t.All", s.encode("t.All", input)), input, "All round-trip");
});

test("protobuf: repeated packed/expanded, maps, oneof", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema(`
    syntax = "proto3";
    message M {
      repeated int32 nums = 1;
      repeated string tags = 2;
      map<string, int32> counts = 3;
      oneof choice { int32 a = 5; string b = 6; }
    }
  `);
  const input = { nums: [1, 2, 300, -4], tags: ["x", "y"], counts: { x: 1, y: 2 }, b: "picked" };
  deepEq(s.decode("M", s.encode("M", input)), input, "repeated/map/oneof round-trip");
});

test("protobuf: implicit presence omits defaults; edition 2023 keeps them", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const p3 = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  if (p3.encode("M", { a: 0 }).length !== 0) throw new Error("proto3 default should be omitted");
  const ed = new Protobuf.Schema(`edition="2023"; message M { int32 a = 1; }`);
  deepEq(Array.from(ed.encode("M", { a: 0 })), [0x08, 0x00], "edition 2023 explicit presence");
  const ed24 = new Protobuf.Schema(`edition="2024"; message M { int32 a = 1; }`);
  deepEq(Array.from(ed24.encode("M", { a: 0 })), [0x08, 0x00], "edition 2024 explicit presence");
});

test("protobuf: unknown fields preserved across re-encode", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const full = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const partial = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  const original = full.encode("M", { a: 5, b: "keep" });
  const reencoded = partial.encode("M", partial.decode("M", original));
  deepEq(full.decode("M", reencoded), { a: 5, b: "keep" }, "unknown field survives");
});

test("protobuf: rejects proto2", async () => {
  const { Protobuf } = await import("runtime:serialization");
  let threw = false;
  try { new Protobuf.Schema(`syntax="proto2"; message M {}`); } catch { threw = true; }
  if (!threw) throw new Error("proto2 should be rejected");
});

test("protobuf: proto3-JSON scalar mapping round-trips", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema(`
    syntax="proto3";
    enum Color { RED = 0; GREEN = 1; }
    message M { int64 big = 1; bytes by = 2; Color c = 3; repeated int32 ns = 4; }
  `);
  const value = { big: 9007199254740993n, by: new Uint8Array([1, 2, 255]), c: "GREEN", ns: [1, 2] };
  const json = s.toJson("M", value);
  deepEq(json, { big: "9007199254740993", by: "AQL/", c: "GREEN", ns: [1, 2] }, "to proto3-JSON");
  deepEq(s.fromJson("M", json), value, "from proto3-JSON");
});

test("protobuf: proto3-JSON well-known types", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema({ "m.proto": `syntax="proto3";
    import "google/protobuf/timestamp.proto";
    import "google/protobuf/struct.proto";
    import "google/protobuf/wrappers.proto";
    message M {
      google.protobuf.Timestamp at = 1;
      google.protobuf.Struct data = 2;
      google.protobuf.Int64Value n = 3;
    }` });
  const json = { at: "1970-01-01T00:00:01Z", data: { a: 1, b: [true, null] }, n: "7" };
  deepEq(s.toJson("M", s.fromJson("M", json)), json, "WKT JSON round-trip");
});

test("protobuf: CLOSED enum retains unrecognized value as unknown field", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const open = new Protobuf.Schema(`edition="2023"; package t; enum E { A=0; B=1; } message M { E e = 1; }`);
  const closed = new Protobuf.Schema(`edition="2023"; package t; enum E { option features.enum_type = CLOSED; A=0; B=1; } message M { E e = 1; }`);
  const wire = open.encode("t.M", { e: 5 }); // 5 is not a declared member
  const decoded = closed.decode("t.M", wire);
  deepEq(decoded.e, undefined, "unknown CLOSED value not surfaced");
  deepEq(Array.from(closed.encode("t.M", decoded)), [0x08, 0x05], "preserved on re-encode");
  deepEq(closed.decode("t.M", open.encode("t.M", { e: 1 })), { e: "B" }, "known value decodes");
});

test("protobuf: decode enforces a maximum nesting depth", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const s = new Protobuf.Schema(`syntax="proto3"; message M { M m = 1; }`);
  let deep = {};
  for (let i = 0; i < 200; i++) deep = { m: deep };
  let threw = false;
  try { s.decode("M", s.encode("M", deep)); } catch (e) { threw = /depth/.test(e.message); }
  deepEq(threw, true, "deep nesting rejected");
});
