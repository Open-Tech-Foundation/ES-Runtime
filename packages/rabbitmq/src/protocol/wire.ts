/**
 * AMQP 0-9-1 wire primitives: the two string forms, the big-endian integers,
 * and the reader/writer every frame is built from.
 *
 * Multi-octet integers are always network byte order (the spec says so, and
 * `DataView` defaults to big-endian). Two string forms exist and the
 * difference is the length prefix: a *short string* is one octet of length
 * (at most 255 bytes), a *long string* four. A long string is bytes rather
 * than text — a header value or a message body is binary-safe — so the reader
 * hands back bytes and only `longstr()` decodes.
 *
 * The reader is bounds-checked on every field: a short frame from a confused
 * peer is a `ProtocolError`, never a read past the end of the buffer.
 */
import { ProtocolError } from "./errors.js";

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

/** A cursor over a byte range, reading fields in order. */
export class ByteReader {
  #bytes: Uint8Array;
  #view: DataView;
  #at = 0;

  constructor(bytes: Uint8Array) {
    this.#bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  /** Bytes consumed so far. */
  get offset(): number {
    return this.#at;
  }

  /** Bytes left unread. */
  get remaining(): number {
    return this.#bytes.length - this.#at;
  }

  /** Whether every byte has been read. */
  get done(): boolean {
    return this.#at >= this.#bytes.length;
  }

  #take(n: number): number {
    const at = this.#at;
    if (n < 0 || at + n > this.#bytes.length) {
      throw new ProtocolError(
        `a field needs ${n} bytes but only ${this.#bytes.length - at} remain`,
      );
    }
    this.#at = at + n;
    return at;
  }

  u8(): number {
    return this.#view.getUint8(this.#take(1));
  }

  i8(): number {
    return this.#view.getInt8(this.#take(1));
  }

  u16(): number {
    return this.#view.getUint16(this.#take(2));
  }

  i16(): number {
    return this.#view.getInt16(this.#take(2));
  }

  u32(): number {
    return this.#view.getUint32(this.#take(4));
  }

  i32(): number {
    return this.#view.getInt32(this.#take(4));
  }

  u64(): bigint {
    return this.#view.getBigUint64(this.#take(8));
  }

  i64(): bigint {
    return this.#view.getBigInt64(this.#take(8));
  }

  f32(): number {
    return this.#view.getFloat32(this.#take(4));
  }

  f64(): number {
    return this.#view.getFloat64(this.#take(8));
  }

  /** The next `n` raw bytes, as a view over the underlying buffer. */
  bytes(n: number): Uint8Array {
    const at = this.#take(n);
    return this.#bytes.subarray(at, at + n);
  }

  /** A short string: one octet of length, decoded as UTF-8. */
  shortstr(): string {
    return DECODER.decode(this.bytes(this.u8()));
  }

  /** A long string, decoded as UTF-8. */
  longstr(): string {
    return DECODER.decode(this.longBytes());
  }

  /** A long string's bytes, without decoding — it may not be text. */
  longBytes(): Uint8Array {
    return this.bytes(this.u32());
  }
}

/**
 * A growable buffer being built up.
 *
 * `finish()` returns a view over the live buffer rather than a copy, so a
 * writer must not be reused after it. That is the whole usage: one writer
 * builds one frame and is dropped.
 */
export class ByteWriter {
  #buf: Uint8Array;
  #view: DataView;
  #at = 0;

  constructor(capacity = 64) {
    this.#buf = new Uint8Array(capacity);
    this.#view = new DataView(this.#buf.buffer);
  }

  /** How many bytes have been written. */
  get length(): number {
    return this.#at;
  }

  #room(n: number): void {
    if (this.#at + n <= this.#buf.length) return;
    let size = Math.max(64, this.#buf.length * 2);
    while (size < this.#at + n) size *= 2;
    const grown = new Uint8Array(size);
    grown.set(this.#buf.subarray(0, this.#at));
    this.#buf = grown;
    this.#view = new DataView(grown.buffer);
  }

  u8(v: number): this {
    this.#room(1);
    this.#view.setUint8(this.#at, v);
    this.#at += 1;
    return this;
  }

  i8(v: number): this {
    this.#room(1);
    this.#view.setInt8(this.#at, v);
    this.#at += 1;
    return this;
  }

  u16(v: number): this {
    this.#room(2);
    this.#view.setUint16(this.#at, v);
    this.#at += 2;
    return this;
  }

  i16(v: number): this {
    this.#room(2);
    this.#view.setInt16(this.#at, v);
    this.#at += 2;
    return this;
  }

  u32(v: number): this {
    this.#room(4);
    this.#view.setUint32(this.#at, v);
    this.#at += 4;
    return this;
  }

  i32(v: number): this {
    this.#room(4);
    this.#view.setInt32(this.#at, v);
    this.#at += 4;
    return this;
  }

  u64(v: bigint): this {
    this.#room(8);
    this.#view.setBigUint64(this.#at, v);
    this.#at += 8;
    return this;
  }

  i64(v: bigint): this {
    this.#room(8);
    this.#view.setBigInt64(this.#at, v);
    this.#at += 8;
    return this;
  }

  f32(v: number): this {
    this.#room(4);
    this.#view.setFloat32(this.#at, v);
    this.#at += 4;
    return this;
  }

  f64(v: number): this {
    this.#room(8);
    this.#view.setFloat64(this.#at, v);
    this.#at += 8;
    return this;
  }

  /** Writes bytes verbatim. */
  raw(bytes: Uint8Array): this {
    this.#room(bytes.length);
    this.#buf.set(bytes, this.#at);
    this.#at += bytes.length;
    return this;
  }

  /** A short string; more than 255 UTF-8 bytes is an error, not a truncation. */
  shortstr(value: string): this {
    const bytes = ENCODER.encode(value);
    if (bytes.length > 255) {
      throw new ProtocolError(`a short string is at most 255 bytes; this one is ${bytes.length}`);
    }
    return this.u8(bytes.length).raw(bytes);
  }

  /** A long string, from text or bytes. */
  longstr(value: string | Uint8Array): this {
    const bytes = typeof value === "string" ? ENCODER.encode(value) : value;
    return this.u32(bytes.length).raw(bytes);
  }

  /**
   * Reserves a 32-bit length to be back-filled once the body's size is known —
   * the shape both a field table and a field array have.
   */
  beginLength(): number {
    const at = this.#at;
    this.u32(0);
    return at;
  }

  /** Back-fills a length reserved by [`beginLength`], excluding the four bytes. */
  endLength(at: number): void {
    this.#view.setUint32(at, this.#at - at - 4);
  }

  /** The written bytes. A view, not a copy — see the class note. */
  finish(): Uint8Array {
    return this.#buf.subarray(0, this.#at);
  }
}
