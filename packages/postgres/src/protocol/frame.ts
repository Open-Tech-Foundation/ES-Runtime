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
  // Kept alongside the buffer rather than made per message: a view is an
  // allocation, and a result set is hundreds of messages.
  #view: DataView;
  #start = 0;
  #end = 0;
  #eof = false;

  constructor(stream: ReadableStream<Uint8Array>, capacity: number = 64 * 1024) {
    this.#reader = stream.getReader();
    this.#buf = new Uint8Array(capacity);
    this.#view = new DataView(this.#buf.buffer);
  }

  get buffered(): number {
    return this.#end - this.#start;
  }

  /** Pulls until `n` bytes are buffered, or the peer hangs up. */
  async #need(n: number): Promise<void> {
    while (this.buffered < n) await this.#fill();
  }

  /** Reads one more chunk off the socket, whatever size it happens to be. */
  async #fill(): Promise<void> {
    if (this.#eof) {
      throw new Error("the connection closed while a message was in flight");
    }
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
    for (;;) {
      const message = this.poll();
      if (message !== null) return message;
      await this.#fill();
    }
  }

  /**
   * The next message if it has already arrived whole, or `null` — without a
   * promise. A result set is usually in the buffer long before anyone asks for
   * it, and waiting a microtask per message to be told so is most of what
   * reading it would cost.
   */
  poll(): { tag: number; frame: Uint8Array } | null {
    const length = this.#complete(this.#start);
    if (length < 0) return null;
    const tag = this.#buf[this.#start]!;
    const frame = this.#buf.subarray(this.#start + 1, this.#start + 1 + length);
    this.#start += 1 + length;
    return { tag, frame };
  }

  /**
   * Moves every message tagged `tag` that has already arrived into `batch`,
   * stopping at the first that is not one, has not arrived whole, or once
   * `batch` holds `limit` bytes. Synchronous, and copies each frame once.
   *
   * This is the row path: a `DataRow` frame is already the shared row layout,
   * so a result set goes from the socket's buffer into the batch the caller
   * decodes with no message objects and no promises in between.
   */
  take(tag: number, batch: FrameBatch, limit: number): void {
    const buf = this.#buf;
    let at = this.#start;
    while (at < this.#end && buf[at] === tag && batch.size < limit) {
      const length = this.#complete(at);
      if (length < 0) break;
      batch.append(buf, at + 1, length);
      at += 1 + length;
    }
    this.#start = at;
  }

  /** The length of the message at `at` if all of it is buffered, else -1. */
  #complete(at: number): number {
    const available = this.#end - at;
    if (available < 5) return -1;
    const length = this.#view.getInt32(at + 1);
    if (length < 4) throw new Error(`a message declared a length of ${length}`);
    return available < 1 + length ? -1 : length;
  }

  async cancel(): Promise<void> {
    try {
      await this.#reader.cancel();
    } catch {
      /* the socket is going away regardless */
    }
  }
}

/**
 * Frames gathered into one buffer, back to back — a batch of rows in the shared
 * layout `runtime:db` decodes.
 *
 * Every batch is a fresh buffer: rows keep a reference to it, so reusing one
 * would silently rewrite rows a caller still holds.
 */
export class FrameBatch {
  bytes: Uint8Array;
  size = 0;
  count = 0;

  constructor(capacity: number) {
    this.bytes = new Uint8Array(Math.max(capacity, 256));
  }

  append(source: Uint8Array, start: number, length: number): void {
    if (this.size + length > this.bytes.length) {
      let capacity = this.bytes.length * 2;
      while (capacity < this.size + length) capacity *= 2;
      const grown = new Uint8Array(capacity);
      grown.set(this.bytes.subarray(0, this.size));
      this.bytes = grown;
    }
    this.bytes.set(source.subarray(start, start + length), this.size);
    this.size += length;
    this.count++;
  }

  /** What was gathered, as a view — the spare capacity is not the caller's. */
  get gathered(): Uint8Array {
    return this.bytes.subarray(0, this.size);
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
