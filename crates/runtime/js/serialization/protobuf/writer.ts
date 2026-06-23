// Low-level protobuf wire writer. Growable buffer; nested messages are encoded
// into child writers and spliced with a length prefix (simple for a reflective
// encoder). Strings via inlined UTF-8.
import { WIRE_LEN } from "./reader.js";
import { utf8Length, utf8Write } from "./utf8.js";

export class Writer {
  private buf: Uint8Array;
  private len: number;
  private view: DataView;

  constructor(capacity = 64) {
    this.buf = new Uint8Array(capacity);
    this.len = 0;
    this.view = new DataView(this.buf.buffer);
  }

  private ensure(n: number): void {
    const need = this.len + n;
    if (need <= this.buf.length) return;
    let cap = this.buf.length * 2;
    while (cap < need) cap *= 2;
    const next = new Uint8Array(cap);
    next.set(this.buf.subarray(0, this.len));
    this.buf = next;
    this.view = new DataView(this.buf.buffer);
  }

  finish(): Uint8Array {
    return this.buf.slice(0, this.len);
  }

  get length(): number {
    return this.len;
  }

  /** Unsigned varint for a value that fits in 32 bits (tags, lengths, etc.). */
  uint32(value: number): void {
    value = value >>> 0;
    this.ensure(5);
    while (value > 0x7f) {
      this.buf[this.len++] = (value & 0x7f) | 0x80;
      value >>>= 7;
    }
    this.buf[this.len++] = value;
  }

  int32(value: number): void {
    // Negative int32 is sign-extended to 64 bits on the wire (10 bytes).
    if (value < 0) this.varint64(BigInt.asUintN(64, BigInt(value)));
    else this.uint32(value);
  }

  sint32(value: number): void {
    this.uint32(((value << 1) ^ (value >> 31)) >>> 0);
  }

  varint64(value: bigint): void {
    value = BigInt.asUintN(64, value);
    this.ensure(10);
    while (value > 0x7fn) {
      this.buf[this.len++] = Number(value & 0x7fn) | 0x80;
      value >>= 7n;
    }
    this.buf[this.len++] = Number(value);
  }

  sint64(value: bigint): void {
    const v = BigInt.asIntN(64, value);
    this.varint64((v << 1n) ^ (v >> 63n));
  }

  bool(value: boolean): void {
    this.ensure(1);
    this.buf[this.len++] = value ? 1 : 0;
  }

  tag(fieldNo: number, wireType: number): void {
    this.uint32(((fieldNo << 3) | wireType) >>> 0);
  }

  fixed32(value: number): void {
    this.ensure(4);
    this.view.setUint32(this.len, value >>> 0, true);
    this.len += 4;
  }

  sfixed32(value: number): void {
    this.ensure(4);
    this.view.setInt32(this.len, value | 0, true);
    this.len += 4;
  }

  float(value: number): void {
    this.ensure(4);
    this.view.setFloat32(this.len, value, true);
    this.len += 4;
  }

  fixed64(value: bigint): void {
    this.ensure(8);
    this.view.setBigUint64(this.len, BigInt.asUintN(64, value), true);
    this.len += 8;
  }

  sfixed64(value: bigint): void {
    this.ensure(8);
    this.view.setBigInt64(this.len, BigInt.asIntN(64, value), true);
    this.len += 8;
  }

  double(value: number): void {
    this.ensure(8);
    this.view.setFloat64(this.len, value, true);
    this.len += 8;
  }

  string(value: string): void {
    const n = utf8Length(value);
    this.uint32(n);
    this.ensure(n);
    this.len = utf8Write(value, this.buf, this.len);
  }

  bytes(value: Uint8Array): void {
    this.uint32(value.length);
    this.ensure(value.length);
    this.buf.set(value, this.len);
    this.len += value.length;
  }

  /** Writes raw pre-encoded bytes verbatim (unknown-field passthrough). */
  raw(value: Uint8Array): void {
    this.ensure(value.length);
    this.buf.set(value, this.len);
    this.len += value.length;
  }

  /** Writes a length-delimited field from a child writer's contents. */
  lenDelimited(fieldNo: number, child: Writer): void {
    this.tag(fieldNo, WIRE_LEN);
    const bytes = child.finish();
    this.uint32(bytes.length);
    this.raw(bytes);
  }
}
