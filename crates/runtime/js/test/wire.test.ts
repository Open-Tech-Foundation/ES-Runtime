import { Reader } from "../serialization/protobuf/reader.ts";
import { Writer } from "../serialization/protobuf/writer.ts";

test("uint32 varint round-trip incl boundaries", () => {
  for (const v of [0, 1, 127, 128, 300, 16383, 16384, 0xffffffff]) {
    const w = new Writer();
    w.uint32(v);
    assertEquals(new Reader(w.finish()).uint32(), v >>> 0);
  }
});

test("int32 negative sign-extends and reads back", () => {
  const w = new Writer();
  w.int32(-7);
  assertEquals(new Reader(w.finish()).int32(), -7);
});

test("sint32 zigzag round-trip", () => {
  for (const v of [0, -1, 1, -123, 2147483647, -2147483648]) {
    const w = new Writer();
    w.sint32(v);
    assertEquals(new Reader(w.finish()).sint32(), v);
  }
});

test("varint64 / int64 / uint64 / sint64 BigInt round-trip", () => {
  const cases: bigint[] = [
    0n,
    1n,
    -1n,
    9007199254740993n,
    -9007199254740993n,
    18446744073709551615n,
    -9223372036854775808n,
  ];
  for (const v of cases) {
    let w = new Writer();
    w.varint64(v);
    assertEquals(new Reader(w.finish()).uint64(), BigInt.asUintN(64, v));

    w = new Writer();
    w.sint64(v);
    assertEquals(new Reader(w.finish()).sint64(), BigInt.asIntN(64, v));
  }
  const w = new Writer();
  w.varint64(BigInt.asUintN(64, -9223372036854775808n));
  assertEquals(new Reader(w.finish()).int64(), -9223372036854775808n);
});

test("fixed widths round-trip", () => {
  const w = new Writer();
  w.fixed32(0xdeadbeef);
  w.sfixed32(-42);
  w.float(1.5);
  w.fixed64(18446744073709551615n);
  w.sfixed64(-99n);
  w.double(44.95);
  const r = new Reader(w.finish());
  assertEquals(r.fixed32(), 0xdeadbeef);
  assertEquals(r.sfixed32(), -42);
  assertEquals(r.float(), 1.5);
  assertEquals(r.fixed64(), 18446744073709551615n);
  assertEquals(r.sfixed64(), -99n);
  assertEquals(r.double(), 44.95);
});

test("string (multibyte) and bytes round-trip", () => {
  const w = new Writer();
  w.string('héllo 𐍈 "q"\n');
  w.bytes(new Uint8Array([1, 2, 3, 255]));
  const r = new Reader(w.finish());
  assertEquals(r.string(), 'héllo 𐍈 "q"\n');
  assertEquals([...r.bytes()], [1, 2, 3, 255]);
});
