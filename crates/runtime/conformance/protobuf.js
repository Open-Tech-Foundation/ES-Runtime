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
  const bytes = s.build("M", { a: 150, b: "hi" });
  deepEq(Array.from(bytes), [0x08, 0x96, 0x01, 0x12, 0x02, 0x68, 0x69], "wire bytes");
  deepEq(s.parse("M", bytes), { a: 150, b: "hi" }, "round-trip");
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
  deepEq(s.parse("t.All", s.build("t.All", input)), input, "All round-trip");
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
  deepEq(s.parse("M", s.build("M", input)), input, "repeated/map/oneof round-trip");
});

test("protobuf: implicit presence omits defaults; edition 2023 keeps them", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const p3 = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  if (p3.build("M", { a: 0 }).length !== 0) throw new Error("proto3 default should be omitted");
  const ed = new Protobuf.Schema(`edition="2023"; message M { int32 a = 1; }`);
  deepEq(Array.from(ed.build("M", { a: 0 })), [0x08, 0x00], "edition 2023 explicit presence");
});

test("protobuf: unknown fields preserved across re-encode", async () => {
  const { Protobuf } = await import("runtime:serialization");
  const full = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; string b = 2; }`);
  const partial = new Protobuf.Schema(`syntax="proto3"; message M { int32 a = 1; }`);
  const original = full.build("M", { a: 5, b: "keep" });
  const reencoded = partial.build("M", partial.parse("M", original));
  deepEq(full.parse("M", reencoded), { a: 5, b: "keep" }, "unknown field survives");
});

test("protobuf: rejects proto2", async () => {
  const { Protobuf } = await import("runtime:serialization");
  let threw = false;
  try { new Protobuf.Schema(`syntax="proto2"; message M {}`); } catch { threw = true; }
  if (!threw) throw new Error("proto2 should be rejected");
});
