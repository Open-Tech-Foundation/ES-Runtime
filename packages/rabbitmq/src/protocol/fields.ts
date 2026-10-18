/**
 * AMQP 0-9-1 field tables, field arrays, and the tagged values they hold.
 *
 * A table is what a method uses for anything free-form — `basic.publish`'s
 * `headers`, a `queue.declare`'s arguments — and it is the one place AMQP is
 * self-describing on the wire: every value carries an ASCII tag octet saying
 * what it is. The tags are RabbitMQ's, which are **not** the ones printed in
 * the 0-9-1 specification: the two disagree, and RabbitMQ follows the Qpid
 * convention it has always implemented (`amqp-0-9-1-errata` §3). Two of those
 * disagreements are load-bearing here and are why every case is written out by
 * hand rather than derived from the spec's grammar:
 *
 * - **`s` is a signed 16-bit integer, not a short string.** Inside a table
 *   RabbitMQ never sends a short string, so a client that reads `s` as one
 *   desynchronises on the first `s`-tagged value.
 * - **`l` and `L` are both signed 64-bit.** The spec calls `L` unsigned; the
 *   broker decodes both as signed, so both decode to `int64` here, and `int64`
 *   is written as `l`.
 *
 * Values cross as a tagged union rather than as bare JavaScript values, because
 * the tag is information the wire carries and JavaScript does not: `int32` and
 * `double` are both `number`, `int64` is a `bigint`, a timestamp is seconds
 * since the epoch, and `void` is a value with no bytes. A caller that knows
 * which it meant can round-trip it exactly.
 */
import { ProtocolError } from "./errors.js";
import type { ByteReader, ByteWriter } from "./wire.js";

/** A table's value: an ordered map, since the wire is an ordered list of pairs. */
export type FieldTable = Map<string, FieldValue>;

/** One tagged field value. */
export type FieldValue =
  | { type: "boolean"; value: boolean }
  | { type: "int8"; value: number }
  | { type: "uint8"; value: number }
  | { type: "int16"; value: number }
  | { type: "uint16"; value: number }
  | { type: "int32"; value: number }
  | { type: "uint32"; value: number }
  // Both `l` and `L` decode to this; it is always written as `l`.
  | { type: "int64"; value: bigint }
  | { type: "float"; value: number }
  | { type: "double"; value: number }
  // A fixed-point number: `unscaled` × 10^-`scale`. Kept exact, as a bigint,
  // because the value AMQP means is the integer and its scale, not a double.
  | { type: "decimal"; scale: number; unscaled: bigint }
  | { type: "string"; value: string }
  | { type: "bytes"; value: Uint8Array }
  | { type: "array"; value: FieldValue[] }
  | { type: "table"; value: FieldTable }
  | { type: "timestamp"; seconds: bigint }
  | { type: "void" };

/** Reads one tagged value. */
export function readFieldValue(r: ByteReader): FieldValue {
  const tag = String.fromCharCode(r.u8());
  switch (tag) {
    case "t":
      return { type: "boolean", value: r.u8() !== 0 };
    case "b":
      return { type: "int8", value: r.i8() };
    case "B":
      return { type: "uint8", value: r.u8() };
    case "s":
      return { type: "int16", value: r.i16() };
    case "u":
      return { type: "uint16", value: r.u16() };
    case "I":
      return { type: "int32", value: r.i32() };
    case "i":
      return { type: "uint32", value: r.u32() };
    case "l":
    case "L":
      return { type: "int64", value: r.i64() };
    case "f":
      return { type: "float", value: r.f32() };
    case "d":
      return { type: "double", value: r.f64() };
    case "D":
      return { type: "decimal", scale: r.u8(), unscaled: BigInt(r.i32()) };
    case "S":
      return { type: "string", value: r.longstr() };
    case "x":
      return { type: "bytes", value: r.longBytes() };
    case "A":
      return { type: "array", value: readFieldArray(r) };
    case "F":
      return { type: "table", value: readFieldTable(r) };
    case "T":
      return { type: "timestamp", seconds: r.u64() };
    case "V":
      return { type: "void" };
    default:
      throw new ProtocolError(`unknown field-value tag ${JSON.stringify(tag)}`);
  }
}

/** Writes one tagged value. */
export function writeFieldValue(w: ByteWriter, v: FieldValue): void {
  switch (v.type) {
    case "boolean":
      w.u8(0x74).u8(v.value ? 1 : 0); // 't'
      break;
    case "int8":
      w.u8(0x62).i8(v.value); // 'b'
      break;
    case "uint8":
      w.u8(0x42).u8(v.value); // 'B'
      break;
    case "int16":
      w.u8(0x73).i16(v.value); // 's'
      break;
    case "uint16":
      w.u8(0x75).u16(v.value); // 'u'
      break;
    case "int32":
      w.u8(0x49).i32(v.value); // 'I'
      break;
    case "uint32":
      w.u8(0x69).u32(v.value); // 'i'
      break;
    case "int64":
      w.u8(0x6c).i64(v.value); // 'l'
      break;
    case "float":
      w.u8(0x66).f32(v.value); // 'f'
      break;
    case "double":
      w.u8(0x64).f64(v.value); // 'd'
      break;
    case "decimal": {
      if (v.unscaled < MIN_I32 || v.unscaled > MAX_I32) {
        throw new ProtocolError(`a decimal's unscaled value must fit in an int32: ${v.unscaled}`);
      }
      w.u8(0x44).u8(v.scale).i32(Number(v.unscaled)); // 'D'
      break;
    }
    case "string":
      w.u8(0x53).longstr(v.value); // 'S'
      break;
    case "bytes":
      w.u8(0x78).longstr(v.value); // 'x'
      break;
    case "array":
      w.u8(0x41); // 'A'
      writeFieldArray(w, v.value);
      break;
    case "table":
      w.u8(0x46); // 'F'
      writeFieldTable(w, v.value);
      break;
    case "timestamp":
      w.u8(0x54).u64(v.seconds); // 'T'
      break;
    case "void":
      w.u8(0x56); // 'V'
      break;
  }
}

const MIN_I32 = -2147483648n;
const MAX_I32 = 2147483647n;

/** Reads a length-prefixed field table. */
export function readFieldTable(r: ByteReader): FieldTable {
  const length = takeLength(r, "table");
  const end = r.offset + length;
  const table: FieldTable = new Map();
  while (r.offset < end) {
    table.set(r.shortstr(), readFieldValue(r));
  }
  if (r.offset !== end) {
    throw new ProtocolError("a field table reads past the length it declared");
  }
  return table;
}

/** Writes a length-prefixed field table. */
export function writeFieldTable(w: ByteWriter, table: FieldTable): void {
  const at = w.beginLength();
  for (const [name, value] of table) {
    w.shortstr(name);
    writeFieldValue(w, value);
  }
  w.endLength(at);
}

/** Reads a length-prefixed field array. */
export function readFieldArray(r: ByteReader): FieldValue[] {
  const length = takeLength(r, "array");
  const end = r.offset + length;
  const items: FieldValue[] = [];
  while (r.offset < end) {
    items.push(readFieldValue(r));
  }
  if (r.offset !== end) {
    throw new ProtocolError("a field array reads past the length it declared");
  }
  return items;
}

/** Writes a length-prefixed field array. */
export function writeFieldArray(w: ByteWriter, items: readonly FieldValue[]): void {
  const at = w.beginLength();
  for (const item of items) {
    writeFieldValue(w, item);
  }
  w.endLength(at);
}

// A declared length that runs past the end of the buffer would make `end`
// larger than the payload; the reads inside would throw anyway, but saying so
// once, with the length, is a better message than "a field needs 4 bytes".
function takeLength(r: ByteReader, what: string): number {
  const length = r.u32();
  if (length > r.remaining) {
    throw new ProtocolError(
      `a field ${what} declares ${length} bytes but only ${r.remaining} remain`,
    );
  }
  return length;
}
