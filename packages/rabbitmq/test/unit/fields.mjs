// Field tables and their tagged values, with no broker anywhere near it.
//
// The cases worth pinning are the ones a live broker will not produce on
// demand: the tag `s`, which the specification and RabbitMQ disagree about, and
// `L`, which every other client reads as unsigned; a table whose declared length
// disagrees with its contents; a tag nothing defines.
import { exit } from "runtime:process";
import {
  readFieldTable,
  readFieldValue,
  writeFieldTable,
  writeFieldValue,
} from "../../dist/protocol/fields.js";
import { ByteReader, ByteWriter } from "../../dist/protocol/wire.js";
import { ok, report } from "./assert.mjs";

/** Structural equality, since `JSON.stringify` cannot see bigints or Maps. */
function same(a, b) {
  if (a === b) return true;
  if (typeof a === "bigint" || typeof b === "bigint") return false;
  if (a instanceof Uint8Array && b instanceof Uint8Array) {
    return a.length === b.length && a.every((byte, i) => byte === b[i]);
  }
  if (a instanceof Map && b instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [key, value] of a) {
      if (!b.has(key) || !same(value, b.get(key))) return false;
    }
    return true;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => same(item, b[i]));
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => same(a[key], b[key]));
  }
  return false;
}

function value(value) {
  const w = new ByteWriter(32);
  writeFieldValue(w, value);
  return readFieldValue(new ByteReader(w.finish()));
}

function table(entries) {
  const w = new ByteWriter(64);
  writeFieldTable(w, new Map(entries));
  return readFieldTable(new ByteReader(w.finish()));
}

// -- the values -------------------------------------------------------------

const VALUES = [
  { type: "boolean", value: true },
  { type: "boolean", value: false },
  { type: "int8", value: -1 },
  { type: "uint8", value: 255 },
  { type: "int16", value: -32768 },
  { type: "uint16", value: 65535 },
  { type: "int32", value: -2147483648 },
  { type: "uint32", value: 4294967295 },
  { type: "int64", value: -9007199254740993n },
  { type: "float", value: 0.5 },
  { type: "double", value: -2.5 },
  { type: "decimal", scale: 2, unscaled: 1234n },
  { type: "string", value: "héllo" },
  { type: "bytes", value: new Uint8Array([0, 1, 255]) },
  { type: "timestamp", seconds: 1700000000n },
  { type: "void" },
];

for (const v of VALUES) {
  ok(same(value(v), v), `a ${v.type} round-trips`);
}

// `s` is the one that bites: the specification prints it as a short string, and
// a client that believes that reads a length octet where the broker wrote half
// of an integer. Pinned with bytes rather than by round-trip, since a codec
// wrong in the same way both ways would still round-trip.
{
  const w = new ByteWriter(8);
  w.u8(0x73).u8(0x01).u8(0x02);
  const v = readFieldValue(new ByteReader(w.finish()));
  ok(same(v, { type: "int16", value: 258 }), "the s tag is a 16-bit integer");
}

// `L` is unsigned in the specification and signed in RabbitMQ; both decode as
// the same signed 64-bit value, and a negative one still arrives negative.
for (const tag of [0x6c, 0x4c]) {
  const w = new ByteWriter(16);
  w.u8(tag).i64(-2n);
  const v = readFieldValue(new ByteReader(w.finish()));
  ok(same(v, { type: "int64", value: -2n }), `the ${String.fromCharCode(tag)} tag is signed`);
}

// And it is always written back as `l`, never `L`.
{
  const w = new ByteWriter(16);
  writeFieldValue(w, { type: "int64", value: 1n });
  ok(w.finish()[0] === 0x6c, "an int64 is written with the l tag");
}

// -- the tables -------------------------------------------------------------

{
  const t = table([
    ["x-death", { type: "array", value: [{ type: "int64", value: 3n }] }],
    ["nested", { type: "table", value: new Map([["k", { type: "string", value: "v" }]]) }],
    ["empty", { type: "table", value: new Map() }],
    ["nothing", { type: "void" }],
  ]);
  ok(
    same(
      t,
      new Map([
        ["x-death", { type: "array", value: [{ type: "int64", value: 3n }] }],
        ["nested", { type: "table", value: new Map([["k", { type: "string", value: "v" }]]) }],
        ["empty", { type: "table", value: new Map() }],
        ["nothing", { type: "void" }],
      ]),
    ),
    "a table of tables and arrays round-trips",
  );
}

// Order is part of the wire format, and a Map that reversed it would change the
// bytes without changing the meaning anyone intended.
{
  const t = table([
    ["b", { type: "int8", value: 1 }],
    ["a", { type: "int8", value: 2 }],
  ]);
  ok([...t.keys()].join(",") === "b,a", "a table keeps insertion order");
}

{
  ok(table([]).size === 0, "an empty table round-trips");
}

// A name longer than 255 UTF-8 bytes cannot be a short string, and truncating
// it would silently address a different key.
{
  const w = new ByteWriter(128);
  let threw = false;
  try {
    writeFieldTable(w, new Map([["x".repeat(256), { type: "void" }]]));
  } catch {
    threw = true;
  }
  ok(threw, "a field name longer than 255 bytes is refused");
}

const BAD_INPUT = [
  ["an unknown tag", new Uint8Array([0x5a, 0x00])],
  ["a truncated value", new Uint8Array([0x49, 0x00, 0x00])],
];

for (const [what, bytes] of BAD_INPUT) {
  let threw = false;
  try {
    readFieldValue(new ByteReader(bytes));
  } catch {
    threw = true;
  }
  ok(threw, `${what} is refused`);
}

// A declared length that runs past what arrived is one error with a length in
// it, not a cascade of short-read failures from inside.
{
  const bytes = new Uint8Array([0x00, 0x00, 0x00, 0x40, 0x01, 0x61]);
  let threw = false;
  try {
    readFieldTable(new ByteReader(bytes));
  } catch {
    threw = true;
  }
  ok(threw, "a table longer than its bytes is refused");
}

// The converse: bytes left over after the declared length mean the pair of
// peers disagree about the encoding, not that the extra is padding.
{
  const w = new ByteWriter(32);
  w.u32(2).shortstr("a").u8(0x56); // 'V', one byte — so the length is 2, one short
  let threw = false;
  try {
    readFieldTable(new ByteReader(w.finish()));
  } catch {
    threw = true;
  }
  ok(threw, "a value past a table's declared length is refused");
}

if (report("fields") > 0) exit(1);
