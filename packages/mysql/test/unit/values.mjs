// Binary rows into the shared layout, and parameters into COM_STMT_EXECUTE —
// the cases a live server will not produce on demand.
import { decodeBatch, defineRowShape } from "runtime:db";
import { exit } from "runtime:process";
import { Writer } from "../../dist/protocol/packets.js";
import { decoderFor, RowBatch, T, widths, writeParams } from "../../dist/protocol/values.js";
import { is, ok, report } from "./assert.mjs";

const columns = [
  { name: "a", type: T.LONGLONG, flags: 0, charset: 63 },
  { name: "b", type: T.VAR_STRING, flags: 0, charset: 255 },
  { name: "c", type: T.TINY, flags: 0x20, charset: 63 },
  { name: "d", type: T.DATETIME, flags: 0, charset: 63 },
  { name: "e", type: T.DOUBLE, flags: 0, charset: 63 },
];

/** A binary row: 0x00, the NULL bitmap (offset 2), then the values. */
function binaryRow(nullColumns, values) {
  const bitmap = new Uint8Array((columns.length + 9) >> 3);
  for (const c of nullColumns) bitmap[(c + 2) >> 3] |= 1 << ((c + 2) & 7);
  const w = new Writer().u8(0).bytes(bitmap);
  values(w);
  return w.finish(0).subarray(4);
}

const rows = [
  binaryRow([], (w) => {
    w.i64(-5n).lenencString("héllo").u8(200);
    w.u8(11).u16(2026).u8(1).u8(2).u8(3).u8(4).u8(5).u32(6);
    w.f64(0.25);
  }),
  binaryRow([1, 3], (w) => {
    w.i64(9007199254740993n).u8(7).f64(-1);
  }),
  // A string long enough for a three-byte length.
  binaryRow([], (w) => {
    w.i64(1n).lenencString("x".repeat(70000)).u8(0).u8(4).u16(1999).u8(12).u8(31).f64(1);
  }),
];

const layout = widths(columns);
is([...layout], [8, 0, 1, -1, 8], "each column's width");
const batch = new RowBatch(0);
for (const row of rows)
  batch.appendBinaryRow(
    row,
    new DataView(row.buffer, row.byteOffset, row.byteLength),
    0,
    row.length,
    layout,
  );
is(batch.count, 3, "three rows transcoded");
const shape = defineRowShape(
  columns.map((c) => ({ name: c.name, declType: null })),
  { decoders: columns.map((c) => decoderFor(c)) },
);
const [one, two, three] = decodeBatch(batch.gathered, shape, batch.count);
is([one.a, one.b, one.c, one.e], [-5, "héllo", 200, 0.25], "numbers, text, unsigned");
is(one.d.toString(), "2026-01-02T03:04:05.000006", "a DATETIME with microseconds");
is(
  [typeof two.a, String(two.a), two.b, two.c, two.d, two.e],
  ["bigint", "9007199254740993", null, 7, null, -1],
  "NULLs from the bitmap",
);
is(
  [three.b.length, three.d.toString()],
  [70000, "1999-12-31T00:00:00"],
  "a long string, and a date-only DATETIME",
);

// Parameters: a NULL bitmap, the types, then the values.
{
  const w = new Writer();
  writeParams(w, [null, 1, "é", 2n ** 64n - 1n, true, 1.5]);
  const bytes = [...w.finish(0).subarray(4)];
  is(bytes.slice(0, 2), [0b1, 1], "NULL bitmap, then 'types follow'");
  is(
    bytes.slice(2, 14),
    [T.NULL, 0, T.LONGLONG, 0, T.VAR_STRING, 0, T.LONGLONG, 0x80, T.TINY, 0, T.DOUBLE, 0],
    "a type per parameter, unsigned flagged",
  );
  is(bytes.slice(14, 22), [1, 0, 0, 0, 0, 0, 0, 0], "an integer is eight bytes little-endian");
  is(bytes.slice(22, 25), [2, 0xc3, 0xa9], "a string is length-prefixed UTF-8");
}
let refused = false;
try {
  writeParams(new Writer(), [2n ** 64n]);
} catch {
  refused = true;
}
ok(refused, "a bigint past 64 bits is refused rather than truncated");

if (report("values") > 0) exit(1);
