import { expect, test } from "bun:test";
import { Reader } from "../serialization/protobuf/reader.js";
import { Writer } from "../serialization/protobuf/writer.js";

test("uint32 varint round-trip incl boundaries", () => {
  for (const v of [0, 1, 127, 128, 300, 16383, 16384, 0xffffffff]) {
    const w = new Writer();
    w.uint32(v);
    expect(new Reader(w.finish()).uint32()).toBe(v >>> 0);
  }
});

test("int32 negative sign-extends and reads back", () => {
  const w = new Writer();
  w.int32(-7);
  expect(new Reader(w.finish()).int32()).toBe(-7);
});

test("sint32 zigzag round-trip", () => {
  for (const v of [0, -1, 1, -123, 2147483647, -2147483648]) {
    const w = new Writer();
    w.sint32(v);
    expect(new Reader(w.finish()).sint32()).toBe(v);
  }
});

test("varint64 / int64 / uint64 / sint64 BigInt round-trip", () => {
  const cases: bigint[] = [0n, 1n, -1n, 9007199254740993n, -9007199254740993n, 18446744073709551615n, -9223372036854775808n];
  for (const v of cases) {
    let w = new Writer();
    w.varint64(v);
    expect(new Reader(w.finish()).uint64()).toBe(BigInt.asUintN(64, v));

    w = new Writer();
    w.sint64(v);
    expect(new Reader(w.finish()).sint64()).toBe(BigInt.asIntN(64, v));
  }
  const w = new Writer();
  w.varint64(BigInt.asUintN(64, -9223372036854775808n));
  expect(new Reader(w.finish()).int64()).toBe(-9223372036854775808n);
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
  expect(r.fixed32()).toBe(0xdeadbeef);
  expect(r.sfixed32()).toBe(-42);
  expect(r.float()).toBe(1.5);
  expect(r.fixed64()).toBe(18446744073709551615n);
  expect(r.sfixed64()).toBe(-99n);
  expect(r.double()).toBe(44.95);
});

test("string (multibyte) and bytes round-trip", () => {
  const w = new Writer();
  w.string('héllo 𐍈 "q"\n');
  w.bytes(new Uint8Array([1, 2, 3, 255]));
  const r = new Reader(w.finish());
  expect(r.string()).toBe('héllo 𐍈 "q"\n');
  expect([...r.bytes()]).toEqual([1, 2, 3, 255]);
});
