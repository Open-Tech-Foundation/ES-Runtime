// Low-level protobuf wire reader. Varints up to 64 bits; 64-bit values surface
// as BigInt. Fixed-width via DataView (little-endian). Strings via inlined UTF-8.
import { utf8Read } from "./utf8.js";

export const WIRE_VARINT = 0;
export const WIRE_I64 = 1;
export const WIRE_LEN = 2;
export const WIRE_SGROUP = 3;
export const WIRE_EGROUP = 4;
export const WIRE_I32 = 5;

export class Reader {
  buf: Uint8Array;
  pos: number;
  end: number;
  private view: DataView;

  constructor(buf: Uint8Array, start = 0, end = buf.length) {
    this.buf = buf;
    this.pos = start;
    this.end = end;
    this.view = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
  }

  eof(): boolean {
    return this.pos >= this.end;
  }

  private need(n: number): void {
    if (this.pos + n > this.end) throw new Error("protobuf: unexpected end of input");
  }

  /** Reads a full varint, returns its low 32 bits as an unsigned number.
   *  Used for tags, lengths, bool, enum, int32/uint32 — and always consumes
   *  the complete varint even if it is wider than 32 bits. Rejects truncated
   *  and overlong (>10 byte) varints. */
  uint32(): number {
    let result = 0;
    let shift = 0;
    let count = 0;
    let b: number;
    do {
      if (this.pos >= this.end) throw new Error("protobuf: unexpected end of input (varint)");
      if (++count > 10) throw new Error("protobuf: varint is too long");
      b = this.buf[this.pos++]!;
      if (shift < 32) result = (result | ((b & 0x7f) << shift)) >>> 0;
      shift += 7;
    } while (b & 0x80);
    return result >>> 0;
  }

  int32(): number {
    return this.uint32() | 0;
  }

  sint32(): number {
    const n = this.uint32();
    return (n >>> 1) ^ -(n & 1);
  }

  /** Reads a full varint as an unsigned 64-bit BigInt. */
  varint64(): bigint {
    let result = 0n;
    let shift = 0n;
    let count = 0;
    let b: number;
    do {
      if (this.pos >= this.end) throw new Error("protobuf: unexpected end of input (varint)");
      if (++count > 10) throw new Error("protobuf: varint is too long");
      b = this.buf[this.pos++]!;
      result |= BigInt(b & 0x7f) << shift;
      shift += 7n;
    } while (b & 0x80);
    return BigInt.asUintN(64, result);
  }

  int64(): bigint {
    return BigInt.asIntN(64, this.varint64());
  }

  uint64(): bigint {
    return this.varint64();
  }

  sint64(): bigint {
    const u = this.varint64();
    return BigInt.asIntN(64, (u >> 1n) ^ -(u & 1n));
  }

  bool(): boolean {
    return this.varint64() !== 0n;
  }

  fixed32(): number {
    this.need(4);
    const v = this.view.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  }

  sfixed32(): number {
    this.need(4);
    const v = this.view.getInt32(this.pos, true);
    this.pos += 4;
    return v;
  }

  float(): number {
    this.need(4);
    const v = this.view.getFloat32(this.pos, true);
    this.pos += 4;
    return v;
  }

  fixed64(): bigint {
    this.need(8);
    const v = this.view.getBigUint64(this.pos, true);
    this.pos += 8;
    return v;
  }

  sfixed64(): bigint {
    this.need(8);
    const v = this.view.getBigInt64(this.pos, true);
    this.pos += 8;
    return v;
  }

  double(): number {
    this.need(8);
    const v = this.view.getFloat64(this.pos, true);
    this.pos += 8;
    return v;
  }

  /** Length-delimited UTF-8 string. */
  string(): string {
    const len = this.uint32();
    this.need(len);
    const start = this.pos;
    this.pos += len;
    return utf8Read(this.buf, start, start + len);
  }

  /** Length-delimited bytes (copied out). */
  bytes(): Uint8Array {
    const len = this.uint32();
    this.need(len);
    const start = this.pos;
    this.pos += len;
    return this.buf.slice(start, start + len);
  }

  /** Returns a sub-reader over the next length-delimited region. */
  fork(): Reader {
    const len = this.uint32();
    this.need(len);
    const start = this.pos;
    this.pos += len;
    return new Reader(this.buf, start, start + len);
  }

  /** Skips a field of the given wire type. `depth` bounds nested group recursion. */
  skip(wireType: number, depth = 0): void {
    switch (wireType) {
      case WIRE_VARINT: {
        let b: number;
        do {
          if (this.pos >= this.end) throw new Error("protobuf: unexpected end of input (varint)");
          b = this.buf[this.pos++]!;
        } while (b & 0x80);
        break;
      }
      case WIRE_I64:
        this.need(8);
        this.pos += 8;
        break;
      case WIRE_LEN: {
        const len = this.uint32();
        this.need(len);
        this.pos += len;
        break;
      }
      case WIRE_I32:
        this.need(4);
        this.pos += 4;
        break;
      case WIRE_SGROUP: {
        // Skip a (deprecated) group: consume fields until the matching end-group.
        if (depth > 100) throw new Error("protobuf: group nesting exceeds maximum depth");
        for (;;) {
          const tag = this.uint32();
          const wt = tag & 7;
          if (wt === WIRE_EGROUP) break;
          this.skip(wt, depth + 1);
        }
        break;
      }
      case WIRE_EGROUP:
        break;
      default:
        throw new Error(`protobuf: cannot skip wire type ${wireType}`);
    }
  }

  /** The raw bytes of a field with the given wire type, starting at `tagStart`
   *  (the position of the tag), for unknown-field preservation. */
  rawFrom(tagStart: number): Uint8Array {
    return this.buf.slice(tagStart, this.pos);
  }
}
