/**
 * RESP — the Redis serialization protocol, versions 2 and 3.
 *
 * Reading and writing, and nothing else: no connection, no commands, no
 * policy. The reader accumulates whatever the socket hands over (64 KiB at a
 * time, with no relationship to reply boundaries) and produces one reply at a
 * time.
 *
 * RESP is *type-tagged per value* rather than described up front the way a
 * `RowDescription` describes a Postgres result. That is the fact that shapes
 * everything here and in the connection above it: a reply's type is known only
 * once it has been read, and an array's elements need not agree with each
 * other.
 */

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** The first byte of a reply, which is its type. */
export const enum Type {
  SimpleString = 0x2b, // +
  Error = 0x2d, // -
  Integer = 0x3a, // :
  Bulk = 0x24, // $
  Array = 0x2a, // *
  // RESP3 adds the rest.
  Null = 0x5f, // _
  Boolean = 0x23, // #
  Double = 0x2c, // ,
  BigNumber = 0x28, // (
  BulkError = 0x21, // !
  Verbatim = 0x3d, // =
  Map = 0x25, // %
  Set = 0x7e, // ~
  Attribute = 0x7c, // |
  Push = 0x3e, // >
}

/**
 * A reply, in the shape the layers above want to look at it.
 *
 * `kind` is kept alongside the value because RESP distinguishes things
 * JavaScript does not. A simple string and a bulk string are both `string`, but
 * only the second is binary-safe and only the first is ever `OK`; a set and an
 * array are both arrays; a map is an array of pairs until someone decides
 * otherwise. Collapsing them at the parser would make those decisions here,
 * where there is no information to make them with.
 */
export type Reply =
  | { kind: "string"; value: string; bytes: Uint8Array; verbatim?: string }
  | { kind: "status"; value: string }
  | { kind: "error"; value: RedisServerError }
  | { kind: "integer"; value: bigint }
  | { kind: "double"; value: number }
  | { kind: "bignumber"; value: bigint }
  | { kind: "boolean"; value: boolean }
  | { kind: "null" }
  | { kind: "array"; value: Reply[] }
  | { kind: "set"; value: Reply[] }
  | { kind: "map"; value: [Reply, Reply][] }
  | { kind: "push"; value: Reply[] };

/** An error reply: `-WRONGTYPE Operation against a key…`. */
export interface RedisServerError {
  /** The leading word, which is Redis's closest thing to an error code. */
  prefix: string;
  message: string;
}

function parseError(text: string): RedisServerError {
  const space = text.indexOf(" ");
  // A bare error with no message is legal, and its whole text is the prefix.
  if (space === -1) return { prefix: text, message: text };
  return { prefix: text.slice(0, space), message: text };
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/**
 * Encodes one command as a RESP array of bulk strings.
 *
 * Every command goes out this way, including `HELLO` and `AUTH` — the inline
 * form exists but has no quoting rules worth trusting a password to, and the
 * unified form is what every server has accepted since 1.2.
 *
 * Arguments are bytes on the wire. A `number` is stringified because that is
 * what Redis parses; a `bigint` likewise, and without the double a `number`
 * would have rounded it through.
 */
export function encodeCommand(args: readonly CommandArg[]): Uint8Array {
  const parts: Uint8Array[] = [];
  let size = 0;
  const push = (bytes: Uint8Array) => {
    parts.push(bytes);
    size += bytes.length;
  };

  push(encoder.encode(`*${args.length}\r\n`));
  for (const arg of args) {
    const bytes = argBytes(arg);
    push(encoder.encode(`$${bytes.length}\r\n`));
    push(bytes);
    push(CRLF);
  }

  const out = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

const CRLF = new Uint8Array([0x0d, 0x0a]);

/** What a command argument may be. Bytes pass through; everything else is text. */
export type CommandArg = string | number | bigint | boolean | Uint8Array | ArrayBufferView | ArrayBuffer;

export function argBytes(arg: CommandArg): Uint8Array {
  if (typeof arg === "string") return encoder.encode(arg);
  if (arg instanceof Uint8Array) return arg;
  if (arg instanceof ArrayBuffer) return new Uint8Array(arg);
  if (ArrayBuffer.isView(arg)) return new Uint8Array(arg.buffer, arg.byteOffset, arg.byteLength);
  if (typeof arg === "bigint") return encoder.encode(arg.toString());
  if (typeof arg === "boolean") return encoder.encode(arg ? "1" : "0");
  if (typeof arg === "number") {
    if (!Number.isFinite(arg)) {
      // Redis spells these, and `String(Infinity)` does not.
      return encoder.encode(arg > 0 ? "+inf" : Number.isNaN(arg) ? "nan" : "-inf");
    }
    return encoder.encode(String(arg));
  }
  throw new TypeError(
    `a Redis argument must be a string, number, bigint, boolean, or bytes — got ${typeof arg}`,
  );
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/** Raised when the peer hangs up mid-reply. There is no recovering a stream. */
export class RespEof extends Error {
  constructor() {
    super("the connection closed while a reply was in flight");
    this.name = "RespEof";
  }
}

/** Raised when the bytes are not RESP at all — a desynchronized stream. */
export class RespProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "RespProtocolError";
  }
}

/**
 * Reads RESP replies out of a byte stream.
 *
 * The buffer slides rather than climbing: a long-lived connection reads far
 * more bytes than it ever holds at once, so it is compacted before it is grown.
 *
 * Unlike a length-prefixed protocol, a RESP reply's size is not known before it
 * is parsed — an array declares its element count, not its bytes — so this
 * parses incrementally against whatever has arrived and asks for more when it
 * runs out, rather than waiting for a complete reply and then parsing it.
 */
export class RespReader {
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

  async #need(n: number): Promise<void> {
    while (this.buffered < n) {
      if (this.#eof) throw new RespEof();
      const { value, done } = await this.#reader.read();
      if (done || value === undefined) {
        this.#eof = true;
        continue;
      }
      this.#append(value);
    }
  }

  #append(chunk: Uint8Array): void {
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

  /** The bytes up to the next CRLF, consuming the terminator. */
  async #line(): Promise<Uint8Array> {
    for (let scan = this.#start; ; ) {
      // Scan only what has not been scanned before: re-scanning from the start
      // of the buffer on every arriving chunk turns one long reply into
      // quadratic work.
      while (scan + 1 < this.#end) {
        if (this.#buf[scan] === 0x0d && this.#buf[scan + 1] === 0x0a) {
          const line = this.#buf.subarray(this.#start, scan);
          this.#start = scan + 2;
          return line;
        }
        scan++;
      }
      const before = this.#end;
      const offset = scan - this.#start;
      await this.#need(this.buffered + 1);
      // `#need` may have compacted the buffer, which moves `#start` — so the
      // scan position is recomputed from its offset rather than carried.
      scan = this.#start + offset;
      if (this.#end === before) throw new RespEof();
    }
  }

  async #number(): Promise<number> {
    const line = await this.#line();
    const text = decoder.decode(line);
    const value = Number(text);
    if (!Number.isFinite(value)) {
      throw new RespProtocolError(`expected a count, read ${JSON.stringify(text)}`);
    }
    return value;
  }

  /** Exactly `n` bytes, then the CRLF that follows them. */
  async #blob(n: number): Promise<Uint8Array> {
    await this.#need(n + 2);
    // Copied, not a view: the caller keeps bulk strings — they are the values —
    // and the next read overwrites this window.
    const bytes = this.#buf.slice(this.#start, this.#start + n);
    this.#start += n + 2;
    return bytes;
  }

  /**
   * The next reply.
   *
   * Attributes (`|`) are read and discarded here rather than surfaced. They are
   * out-of-band metadata attached to a reply — client-side caching hints, and
   * whatever a future server adds — and a client that let them through would
   * hand callers a reply whose shape depended on server configuration.
   */
  async next(): Promise<Reply> {
    for (;;) {
      const reply = await this.#one();
      if (reply === ATTRIBUTE) continue;
      return reply;
    }
  }

  async #one(): Promise<Reply | typeof ATTRIBUTE> {
    await this.#need(1);
    const type = this.#buf[this.#start++]!;
    switch (type) {
      case Type.SimpleString:
        return { kind: "status", value: decoder.decode(await this.#line()) };
      case Type.Error:
        return { kind: "error", value: parseError(decoder.decode(await this.#line())) };
      case Type.BulkError: {
        const n = await this.#number();
        return { kind: "error", value: parseError(decoder.decode(await this.#blob(n))) };
      }
      case Type.Integer:
        // A bigint always, narrowed by the layer that knows whether the caller
        // wanted a number. Redis integers are signed 64-bit and `INCRBY` can
        // reach past 2^53, so parsing to a double here would lose the value
        // before anyone could ask for it exactly.
        return { kind: "integer", value: BigInt(decoder.decode(await this.#line())) };
      case Type.Bulk: {
        const n = await this.#number();
        // RESP2 spells null as a bulk string of length -1, and an array of
        // length -1. Both mean the same absence and both become `null`.
        if (n < 0) return NULL;
        const bytes = await this.#blob(n);
        return { kind: "string", value: decoder.decode(bytes), bytes };
      }
      case Type.Verbatim: {
        const n = await this.#number();
        const bytes = await this.#blob(n);
        // `txt:` or `mkd:` — three characters of format, a colon, then the
        // content. The tag is kept and stripped, because the content is the
        // value and a caller asking for a config file does not want `txt:` on
        // the front of it.
        const text = decoder.decode(bytes);
        return {
          kind: "string",
          value: text.slice(4),
          bytes: bytes.subarray(4),
          verbatim: text.slice(0, 3),
        };
      }
      case Type.Array:
      case Type.Set:
      case Type.Push: {
        const n = await this.#number();
        if (n < 0) return NULL;
        const items: Reply[] = [];
        for (let i = 0; i < n; i++) items.push(await this.next());
        const kind = type === Type.Array ? "array" : type === Type.Set ? "set" : "push";
        return { kind, value: items } as Reply;
      }
      case Type.Map: {
        const n = await this.#number();
        if (n < 0) return NULL;
        const pairs: [Reply, Reply][] = [];
        for (let i = 0; i < n; i++) pairs.push([await this.next(), await this.next()]);
        return { kind: "map", value: pairs };
      }
      case Type.Attribute: {
        const n = await this.#number();
        for (let i = 0; i < n; i++) {
          await this.next();
          await this.next();
        }
        return ATTRIBUTE;
      }
      case Type.Null:
        await this.#line(); // the empty remainder of `_\r\n`
        return NULL;
      case Type.Boolean: {
        const line = decoder.decode(await this.#line());
        return { kind: "boolean", value: line === "t" };
      }
      case Type.Double: {
        const text = decoder.decode(await this.#line());
        // `inf`, `-inf` and `nan` are RESP3's spellings and `Number()` reads
        // none of them.
        const value =
          text === "inf"
            ? Number.POSITIVE_INFINITY
            : text === "-inf"
              ? Number.NEGATIVE_INFINITY
              : text === "nan"
                ? Number.NaN
                : Number(text);
        return { kind: "double", value };
      }
      case Type.BigNumber:
        return { kind: "bignumber", value: BigInt(decoder.decode(await this.#line())) };
      default:
        throw new RespProtocolError(
          `${JSON.stringify(String.fromCharCode(type))} is not a RESP type byte — the stream is out of step`,
        );
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

const NULL: Reply = { kind: "null" };

/** The sentinel that says "this was an attribute; read another". */
const ATTRIBUTE = Symbol("attribute");
