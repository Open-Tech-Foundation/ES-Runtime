// Incremental decode of a repeated message field from a chunked byte source.
// Walks the outer message's wire fields one at a time, yielding each element of
// the target repeated field as it is fully buffered, and skipping every other
// field — so a large collection streams without materializing the whole array.
//
// The source may be a Web ReadableStream, an async iterable, or a sync iterable
// of Uint8Array chunks. A small buffering reader handles values (varints,
// length-delimited regions) that straddle chunk boundaries.
import type { Field, MessageType } from "./descriptor.js";
import { decode } from "./decode.js";
import { Reader, WIRE_EGROUP, WIRE_LEN } from "./reader.js";

interface ReadableStreamLike {
  getReader(): { read(): Promise<{ done: boolean; value?: Uint8Array }> };
}

export type StreamSource =
  | Uint8Array
  | ReadableStreamLike
  | AsyncIterable<Uint8Array>
  | Iterable<Uint8Array>;

/** Normalizes any accepted source into a `pull()` that yields the next chunk or
 *  null at end of input. */
function pullFrom(source: StreamSource): () => Promise<Uint8Array | null> {
  if (source instanceof Uint8Array) {
    let sent = false;
    return async () => (sent ? null : ((sent = true), source));
  }
  const s = source as Record<symbol | string, unknown>;
  if (typeof s.getReader === "function") {
    const r = (source as ReadableStreamLike).getReader();
    return async () => {
      const { done, value } = await r.read();
      return done || value === undefined ? null : value;
    };
  }
  if (typeof s[Symbol.asyncIterator] === "function") {
    const it = (source as AsyncIterable<Uint8Array>)[Symbol.asyncIterator]();
    return async () => {
      const { done, value } = await it.next();
      return done ? null : value;
    };
  }
  if (typeof s[Symbol.iterator] === "function") {
    const it = (source as Iterable<Uint8Array>)[Symbol.iterator]();
    return async () => {
      const { done, value } = it.next();
      return done ? null : value;
    };
  }
  throw new Error("protobuf: decodeStream source must be a ReadableStream or (async) iterable of Uint8Array");
}

/** A pull-driven byte cursor that buffers across chunk boundaries. */
class ByteStream {
  private buf = new Uint8Array(0);
  private pos = 0;
  private done = false;

  constructor(private readonly pull: () => Promise<Uint8Array | null>) {}

  /** Pulls one more chunk, compacting away already-consumed bytes. */
  private async more(): Promise<boolean> {
    if (this.done) return false;
    const chunk = await this.pull();
    if (chunk == null) { this.done = true; return false; }
    if (chunk.length === 0) return this.more();
    const rest = this.buf.subarray(this.pos);
    const next = new Uint8Array(rest.length + chunk.length);
    next.set(rest);
    next.set(chunk, rest.length);
    this.buf = next;
    this.pos = 0;
    return true;
  }

  /** Ensures at least `n` unconsumed bytes are buffered; false at clean EOF. */
  private async ensure(n: number): Promise<boolean> {
    while (this.buf.length - this.pos < n) if (!(await this.more())) return false;
    return true;
  }

  /** True once the input is fully consumed. */
  async atEnd(): Promise<boolean> {
    if (this.buf.length > this.pos) return false;
    return !(await this.more());
  }

  async varint(): Promise<bigint> {
    let result = 0n;
    let shift = 0n;
    let count = 0;
    for (;;) {
      if (this.pos >= this.buf.length && !(await this.more())) {
        throw new Error("protobuf: truncated varint in stream");
      }
      if (++count > 10) throw new Error("protobuf: varint is too long");
      const b = this.buf[this.pos++]!;
      result |= BigInt(b & 0x7f) << shift;
      shift += 7n;
      if (!(b & 0x80)) break;
    }
    return result;
  }

  /** Reads the next field tag, or null at end of input. */
  async tag(): Promise<{ fieldNo: number; wire: number } | null> {
    if (await this.atEnd()) return null;
    const v = Number(await this.varint());
    return { fieldNo: v >>> 3, wire: v & 7 };
  }

  /** Returns the next `n` bytes (a view, valid until the next pull). */
  async bytes(n: number): Promise<Uint8Array> {
    if (!(await this.ensure(n))) throw new Error("protobuf: truncated length-delimited value in stream");
    const out = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }

  /** Consumes a field of the given wire type without decoding it. */
  async skip(wire: number): Promise<void> {
    switch (wire) {
      case 0: await this.varint(); break;
      case 1: await this.bytes(8); break;
      case WIRE_LEN: await this.bytes(Number(await this.varint())); break;
      case 5: await this.bytes(4); break;
      case 3: { // start-group: skip until the matching end-group
        for (;;) {
          const t = await this.tag();
          if (!t) throw new Error("protobuf: truncated group in stream");
          if (t.wire === WIRE_EGROUP) break;
          await this.skip(t.wire);
        }
        break;
      }
      case WIRE_EGROUP: break;
      default: throw new Error(`protobuf: cannot skip wire type ${wire} in stream`);
    }
  }
}

/** Yields each element of `field` (a repeated message field) decoded from the
 *  chunked `source`; all other fields of the outer message are skipped. */
export async function* decodeStream(field: Field, source: StreamSource): AsyncGenerator<Record<string, unknown>> {
  if (field.type.kind !== "message") return;
  const element = field.type.message;
  const reader = new ByteStream(pullFrom(source));
  for (;;) {
    const t = await reader.tag();
    if (!t) break;
    if (t.fieldNo === field.number && t.wire === WIRE_LEN) {
      const len = Number(await reader.varint());
      yield decode(element, new Reader(await reader.bytes(len)));
    } else {
      await reader.skip(t.wire);
    }
  }
}

/** Yields each message of a length-delimited stream — a sequence of
 *  varint-length-prefixed messages (the `writeDelimitedTo` framing). */
export async function* decodeDelimitedStream(message: MessageType, source: StreamSource): AsyncGenerator<Record<string, unknown>> {
  const reader = new ByteStream(pullFrom(source));
  while (!(await reader.atEnd())) {
    const len = Number(await reader.varint());
    yield decode(message, new Reader(await reader.bytes(len)));
  }
}
