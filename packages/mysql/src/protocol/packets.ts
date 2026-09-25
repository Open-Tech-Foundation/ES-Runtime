/**
 * Reading and writing MySQL packets.
 *
 * Every exchange after the socket opens is a run of packets: a three-byte
 * little-endian payload length, a one-byte sequence id, then the payload. The
 * sequence id counts packets within one command and restarts at zero with the
 * next, and a payload of exactly 2^24 - 1 bytes means "continued in the next
 * packet" — which is how a value larger than 16 MiB crosses at all.
 */

/** The largest payload one packet can carry; a payload this long continues. */
export const MAX_PAYLOAD = 0xff_ffff;

const DECODER = new TextDecoder();
const ENCODER = new TextEncoder();

/**
 * Takes a byte stream apart into packets.
 *
 * Bytes arrive in whatever chunks the socket hands over, so this accumulates
 * and hands back one payload at a time. Most of a result set is already in the
 * buffer long before anyone asks for it, so the fast paths — `poll` and `take` —
 * answer from the buffer without a promise, and only a packet that has not
 * arrived whole costs one.
 */
export class PacketReader {
  #reader: ReadableStreamDefaultReader<Uint8Array>;
  #buf: Uint8Array;
  #view: DataView;
  #start = 0;
  #end = 0;
  #eof = false;
  /** The sequence id of the last packet read; the next one written follows it. */
  seq = 0;

  constructor(stream: ReadableStream<Uint8Array>, capacity: number = 64 * 1024) {
    this.#reader = stream.getReader();
    this.#buf = new Uint8Array(capacity);
    this.#view = new DataView(this.#buf.buffer);
  }

  get buffered(): number {
    return this.#end - this.#start;
  }

  /** Reads one more chunk off the socket. */
  async #fill(): Promise<void> {
    if (this.#eof) throw new Error("the connection closed while a packet was in flight");
    const { value, done } = await this.#reader.read();
    if (done || value === undefined) {
      this.#eof = true;
      return;
    }
    this.#append(value);
  }

  #append(chunk: Uint8Array): void {
    // Compact before growing: a long-lived connection reads far more bytes than
    // it ever holds, so the window slides rather than the buffer climbing.
    if (this.#end + chunk.length > this.#buf.length) {
      if (this.buffered + chunk.length <= this.#buf.length) {
        this.#buf.copyWithin(0, this.#start, this.#end);
      } else {
        let size = this.#buf.length * 2;
        while (size < this.buffered + chunk.length) size *= 2;
        const grown = new Uint8Array(size);
        grown.set(this.#buf.subarray(this.#start, this.#end));
        this.#buf = grown;
        this.#view = new DataView(grown.buffer);
      }
      this.#end -= this.#start;
      this.#start = 0;
    }
    this.#buf.set(chunk, this.#end);
    this.#end += chunk.length;
  }

  /** The payload length of the packet at `at` if all of it is buffered, else -1. */
  #complete(at: number): number {
    if (this.#end - at < 4) return -1;
    const length = this.#view.getUint16(at, true) | (this.#buf[at + 2]! << 16);
    return this.#end - at - 4 < length ? -1 : length;
  }

  /**
   * The next payload if it has arrived whole, or `null` — without a promise.
   *
   * A view into the read buffer, valid until the next read. A payload split
   * across continuation packets is joined into a buffer of its own.
   */
  poll(): Uint8Array | null {
    const length = this.#complete(this.#start);
    if (length < 0) return null;
    if (length === MAX_PAYLOAD) return this.#joined();
    this.seq = this.#buf[this.#start + 3]!;
    const payload = this.#buf.subarray(this.#start + 4, this.#start + 4 + length);
    this.#start += 4 + length;
    return payload;
  }

  /** A payload continued across packets, once every part of it is here. */
  #joined(): Uint8Array | null {
    let at = this.#start;
    let total = 0;
    for (;;) {
      const length = this.#complete(at);
      if (length < 0) return null;
      total += length;
      at += 4 + length;
      if (length < MAX_PAYLOAD) break;
    }
    const out = new Uint8Array(total);
    let written = 0;
    while (this.#start < at) {
      const length = this.#complete(this.#start);
      out.set(this.#buf.subarray(this.#start + 4, this.#start + 4 + length), written);
      written += length;
      this.seq = this.#buf[this.#start + 3]!;
      this.#start += 4 + length;
    }
    return out;
  }

  /** The next payload, waiting for it if it has not arrived. */
  async packet(): Promise<Uint8Array> {
    for (;;) {
      const payload = this.poll();
      if (payload !== null) return payload;
      await this.#fill();
    }
  }

  /**
   * Hands every buffered packet that starts with `lead` to `row`, stopping at
   * the first that does not, has not arrived whole, is a continuation, or when
   * `row` returns false. Synchronous.
   *
   * This is the row path: a result set's rows are handed over as spans of the
   * read buffer, with no payload views and no promises in between.
   */
  take(
    lead: number,
    row: (bytes: Uint8Array, view: DataView, start: number, length: number) => boolean,
  ): void {
    const buf = this.#buf;
    for (;;) {
      const at = this.#start;
      const length = this.#complete(at);
      if (length <= 0 || length === MAX_PAYLOAD || buf[at + 4] !== lead) return;
      this.seq = buf[at + 3]!;
      this.#start = at + 4 + length;
      if (!row(buf, this.#view, at + 4, length)) return;
    }
  }

  async cancel(): Promise<void> {
    try {
      await this.#reader.cancel();
    } catch {
      /* the socket is going away regardless */
    }
  }
}

/** Reads fields out of one payload. Integers are little-endian throughout. */
export class Payload {
  readonly bytes: Uint8Array;
  readonly view: DataView;
  at: number;

  constructor(bytes: Uint8Array, at = 0) {
    this.bytes = bytes;
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.at = at;
  }

  get done(): boolean {
    return this.at >= this.bytes.length;
  }

  u8(): number {
    return this.bytes[this.at++]!;
  }

  u16(): number {
    const value = this.view.getUint16(this.at, true);
    this.at += 2;
    return value;
  }

  u24(): number {
    const value = this.view.getUint16(this.at, true) | (this.bytes[this.at + 2]! << 16);
    this.at += 3;
    return value;
  }

  u32(): number {
    const value = this.view.getUint32(this.at, true);
    this.at += 4;
    return value;
  }

  /**
   * A length-encoded integer. Returned as a number: every count and length the
   * protocol sends fits, and an affected-row count past 2^53 is not a real
   * concern. `null` is the `0xFB` marker, which in a text row means NULL.
   */
  lenenc(): number | null {
    const first = this.u8();
    if (first < 0xfb) return first;
    switch (first) {
      case 0xfb:
        return null;
      case 0xfc:
        return this.u16();
      case 0xfd:
        return this.u24();
      case 0xfe: {
        const low = this.u32();
        const high = this.u32();
        return high * 0x1_0000_0000 + low;
      }
      default:
        throw new Error(`0x${first.toString(16)} is not a length-encoded integer`);
    }
  }

  /** A length-encoded integer that must be there — a count, not a value. */
  count(): number {
    return this.lenenc() ?? 0;
  }

  bytesOf(n: number): Uint8Array {
    const slice = this.bytes.subarray(this.at, this.at + n);
    this.at += n;
    return slice;
  }

  /** A length-encoded string. */
  lenencString(): string {
    return DECODER.decode(this.bytesOf(this.count()));
  }

  /** Skips a length-encoded string without decoding it. */
  skipLenenc(): void {
    // Two statements, not `this.at += this.count()`: that reads `this.at`
    // before `count()` has moved it past the length itself.
    const length = this.count();
    this.at += length;
  }

  /** A NUL-terminated string. */
  cstring(): string {
    let end = this.at;
    while (end < this.bytes.length && this.bytes[end] !== 0) end++;
    const text = DECODER.decode(this.bytes.subarray(this.at, end));
    this.at = end + 1;
    return text;
  }

  /** Whatever is left, as text. */
  rest(): string {
    const text = DECODER.decode(this.bytes.subarray(this.at));
    this.at = this.bytes.length;
    return text;
  }

  restBytes(): Uint8Array {
    const bytes = this.bytes.subarray(this.at);
    this.at = this.bytes.length;
    return bytes;
  }
}

/** Builds one payload, then frames it into packets. */
export class Writer {
  #buf: Uint8Array;
  #view: DataView;
  #at = 4; // room for the first packet header

  constructor(capacity = 256) {
    this.#buf = new Uint8Array(Math.max(capacity, 16));
    this.#view = new DataView(this.#buf.buffer);
  }

  #reserve(n: number): void {
    if (this.#at + n <= this.#buf.length) return;
    let size = this.#buf.length * 2;
    while (size < this.#at + n) size *= 2;
    const grown = new Uint8Array(size);
    grown.set(this.#buf.subarray(0, this.#at));
    this.#buf = grown;
    this.#view = new DataView(grown.buffer);
  }

  u8(value: number): this {
    this.#reserve(1);
    this.#buf[this.#at++] = value;
    return this;
  }

  u16(value: number): this {
    this.#reserve(2);
    this.#view.setUint16(this.#at, value, true);
    this.#at += 2;
    return this;
  }

  u32(value: number): this {
    this.#reserve(4);
    this.#view.setUint32(this.#at, value >>> 0, true);
    this.#at += 4;
    return this;
  }

  i64(value: bigint): this {
    this.#reserve(8);
    this.#view.setBigInt64(this.#at, value, true);
    this.#at += 8;
    return this;
  }

  u64(value: bigint): this {
    this.#reserve(8);
    this.#view.setBigUint64(this.#at, value, true);
    this.#at += 8;
    return this;
  }

  f64(value: number): this {
    this.#reserve(8);
    this.#view.setFloat64(this.#at, value, true);
    this.#at += 8;
    return this;
  }

  zeros(n: number): this {
    this.#reserve(n);
    this.#buf.fill(0, this.#at, this.#at + n);
    this.#at += n;
    return this;
  }

  bytes(bytes: Uint8Array): this {
    this.#reserve(bytes.length);
    this.#buf.set(bytes, this.#at);
    this.#at += bytes.length;
    return this;
  }

  lenenc(value: number): this {
    if (value < 0xfb) return this.u8(value);
    if (value <= 0xffff) return this.u8(0xfc).u16(value);
    if (value <= 0xff_ffff)
      return this.u8(0xfd)
        .u16(value & 0xffff)
        .u8(value >>> 16);
    return this.u8(0xfe).u64(BigInt(value));
  }

  lenencBytes(bytes: Uint8Array): this {
    return this.lenenc(bytes.length).bytes(bytes);
  }

  lenencString(text: string): this {
    return this.lenencBytes(ENCODER.encode(text));
  }

  cstring(text: string): this {
    return this.bytes(ENCODER.encode(text)).u8(0);
  }

  string(text: string): this {
    return this.bytes(ENCODER.encode(text));
  }

  /**
   * The payload framed as packets, starting at sequence id `seq`.
   *
   * One packet in the common case, written in place into the room reserved at
   * the front. A payload of 16 MiB or more is split, and one that is an exact
   * multiple of the limit ends with an empty packet — that is how the reader
   * knows it has stopped.
   */
  finish(seq: number): Uint8Array {
    const length = this.#at - 4;
    if (length < MAX_PAYLOAD) {
      this.#header(this.#buf, 0, length, seq);
      return this.#buf.subarray(0, this.#at);
    }
    const parts = Math.floor(length / MAX_PAYLOAD) + 1;
    const out = new Uint8Array(length + parts * 4);
    let read = 4;
    let written = 0;
    for (let i = 0; i < parts; i++) {
      const size = Math.min(MAX_PAYLOAD, this.#at - read);
      this.#header(out, written, size, (seq + i) & 0xff);
      out.set(this.#buf.subarray(read, read + size), written + 4);
      read += size;
      written += 4 + size;
    }
    return out;
  }

  #header(out: Uint8Array, at: number, length: number, seq: number): void {
    out[at] = length & 0xff;
    out[at + 1] = (length >>> 8) & 0xff;
    out[at + 2] = (length >>> 16) & 0xff;
    out[at + 3] = seq & 0xff;
  }
}
