/**
 * MQTT wire primitives: the big-endian integers, the UTF-8 string and binary
 * data forms, the variable byte integer, and the reader and writer every packet
 * is built from.
 *
 * MQTT has three length-prefixed forms and they are worth telling apart. A
 * **UTF-8 Encoded String** is two octets of byte length then UTF-8 text, and
 * carries two rules the specification is strict about: no U+0000 (it is the
 * string's own terminator elsewhere) and well-formed UTF-8. **Binary Data** is
 * the same two-octet prefix with no text rules at all — a will payload or
 * correlation data may be any bytes. A **Variable Byte Integer** (see
 * `varint.ts`) is the third, and it is what carries a packet's length.
 *
 * The reader bounds-checks every field: a short packet from a confused peer is
 * a `ProtocolError`, never a read past the end of the buffer.
 */

import { ProtocolError } from "./errors.js";
import { decodeVarint, encodeVarint, varintLength } from "./varint.js";

const ENCODER = new TextEncoder();
// `fatal` is what turns a malformed UTF-8 sequence into a thrown error rather
// than U+FFFD — the specification requires well-formed input, and silently
// substituting replacement characters would corrupt a topic or a payload.
const DECODER = new TextDecoder("utf-8", { fatal: true });

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

  u16(): number {
    return this.#view.getUint16(this.#take(2));
  }

  u32(): number {
    return this.#view.getUint32(this.#take(4));
  }

  /** The next `n` raw bytes, as a view over the underlying buffer. */
  bytes(n: number): Uint8Array {
    const at = this.#take(n);
    return this.#bytes.subarray(at, at + n);
  }

  /** A variable byte integer. */
  varint(): number {
    const { value, length } = decodeVarint(this.#bytes, this.#at);
    this.#take(length);
    return value;
  }

  /** A UTF-8 Encoded String: two octets of length, then the text. */
  utf8(): string {
    const length = this.u16();
    const raw = this.bytes(length);
    let text: string;
    try {
      text = DECODER.decode(raw);
    } catch {
      throw new ProtocolError("a UTF-8 string was not well-formed UTF-8");
    }
    if (text.includes("\u0000")) {
      throw new ProtocolError("a UTF-8 string must not contain U+0000");
    }
    return text;
  }

  /** Binary Data: two octets of length, then the bytes, with no text rules. */
  binary(): Uint8Array {
    return this.bytes(this.u16());
  }

  /** Every byte that is left. */
  rest(): Uint8Array {
    return this.bytes(this.remaining);
  }

  /** A sub-reader over the next `n` bytes, for a length-delimited section. */
  section(n: number): ByteReader {
    return new ByteReader(this.bytes(n));
  }
}

/**
 * A growable buffer being built up.
 *
 * `finish()` returns a view over the live buffer rather than a copy, so a
 * writer must not be reused after it. That is the whole usage: one writer
 * builds one packet and is dropped.
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

  u16(v: number): this {
    this.#room(2);
    this.#view.setUint16(this.#at, v);
    this.#at += 2;
    return this;
  }

  u32(v: number): this {
    this.#room(4);
    this.#view.setUint32(this.#at, v);
    this.#at += 4;
    return this;
  }

  /** Writes bytes verbatim. */
  raw(bytes: Uint8Array): this {
    this.#room(bytes.length);
    this.#buf.set(bytes, this.#at);
    this.#at += bytes.length;
    return this;
  }

  /** A variable byte integer. Validated, so an out-of-range value is an error. */
  varint(value: number): this {
    varintLength(value);
    this.#room(4);
    const encoded = encodeVarint(value);
    this.#buf.set(encoded, this.#at);
    this.#at += encoded.length;
    return this;
  }

  /** A UTF-8 Encoded String. U+0000 and an over-long value are both errors. */
  utf8(text: string): this {
    if (text.includes("\u0000")) {
      throw new ProtocolError("a UTF-8 string must not contain U+0000");
    }
    const bytes = ENCODER.encode(text);
    if (bytes.length > 65535) {
      throw new ProtocolError(`a UTF-8 string is at most 65535 bytes; this one is ${bytes.length}`);
    }
    return this.u16(bytes.length).raw(bytes);
  }

  /** Binary Data. */
  binary(bytes: Uint8Array): this {
    if (bytes.length > 65535) {
      throw new ProtocolError(`binary data is at most 65535 bytes; this one is ${bytes.length}`);
    }
    return this.u16(bytes.length).raw(bytes);
  }

  /** The bytes written so far, as a view over the live buffer. */
  finish(): Uint8Array {
    return this.#buf.subarray(0, this.#at);
  }
}
