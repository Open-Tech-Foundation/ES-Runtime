/**
 * Reading the wire: length-prefixed frames out of a byte stream.
 *
 * Every backend message after the handshake is `tag(1) length(4) body`, where
 * the length counts itself. The socket hands over whatever arrived — 64 KiB at
 * a time, with no relationship to message boundaries — so this accumulates and
 * hands back one message at a time.
 *
 * The frame is kept **including its length prefix**, because that is exactly
 * the layout `runtime:db`'s row decoder reads: a `DataRow` body is
 * `length(4) columns(2) [len(4) bytes]*`, which is the shared row encoding. So
 * rows are appended to a batch buffer as they arrive and never transcoded.
 */
export class FrameReader {
  #reader: ReadableStreamDefaultReader<Uint8Array>;
  #buf: Uint8Array;
  #start = 0;
  #end = 0;
  #eof = false;

  constructor(stream: ReadableStream<Uint8Array>, capacity: number = 64 * 1024) {
    this.#reader = stream.getReader();
    this.#buf = new Uint8Array(capacity);
  }

  get buffered(): number {
    return this.#end - this.#start;
  }

  /** Pulls until `n` bytes are buffered, or the peer hangs up. */
  async #need(n: number): Promise<void> {
    while (this.buffered < n) {
      if (this.#eof) {
        throw new Error("the connection closed while a message was in flight");
      }
      const { value, done } = await this.#reader.read();
      if (done || value === undefined) {
        this.#eof = true;
        continue;
      }
      this.#append(value);
    }
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
      }
      this.#end -= this.#start;
      this.#start = 0;
    }
    this.#buf.set(chunk, this.#end);
    this.#end += chunk.length;
  }

  /** One raw byte — the server's yes/no answer to `SSLRequest`. */
  async byte(): Promise<number> {
    await this.#need(1);
    return this.#buf[this.#start++]!;
  }

  /**
   * The next message: its tag, and the frame from the length prefix onward.
   *
   * The frame is a **view into the read buffer**, valid only until the next
   * call. A caller that keeps it (the row path does) copies it.
   */
  async message(): Promise<{ tag: number; frame: Uint8Array }> {
    await this.#need(5);
    const tag = this.#buf[this.#start]!;
    const view = new DataView(this.#buf.buffer, this.#buf.byteOffset);
    const length = view.getInt32(this.#start + 1);
    if (length < 4) throw new Error(`a message declared a length of ${length}`);
    await this.#need(1 + length);
    const frame = this.#buf.subarray(this.#start + 1, this.#start + 1 + length);
    this.#start += 1 + length;
    return { tag, frame };
  }

  async cancel(): Promise<void> {
    try {
      await this.#reader.cancel();
    } catch {
      /* the socket is going away regardless */
    }
  }
}

/** Reads fields out of a message body (the frame, past its length prefix). */
export class Fields {
  #view: DataView;
  #bytes: Uint8Array;
  at: number;

  constructor(frame: Uint8Array, at = 4) {
    this.#bytes = frame;
    this.#view = new DataView(frame.buffer, frame.byteOffset, frame.byteLength);
    this.at = at;
  }

  i16(): number {
    const value = this.#view.getInt16(this.at);
    this.at += 2;
    return value;
  }

  i32(): number {
    const value = this.#view.getInt32(this.at);
    this.at += 4;
    return value;
  }

  u8(): number {
    return this.#bytes[this.at++]!;
  }

  /** A null-terminated string. */
  cstring(): string {
    let end = this.at;
    while (end < this.#bytes.length && this.#bytes[end] !== 0) end++;
    const text = DECODER.decode(this.#bytes.subarray(this.at, end));
    this.at = end + 1;
    return text;
  }

  bytes(n: number): Uint8Array {
    const slice = this.#bytes.subarray(this.at, this.at + n);
    this.at += n;
    return slice;
  }

  get done(): boolean {
    return this.at >= this.#bytes.length;
  }
}

const DECODER = new TextDecoder();
