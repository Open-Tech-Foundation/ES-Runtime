/**
 * Frame boundaries: the layer between "bytes arrived from the socket" and "a
 * method, a content header, or a body chunk".
 *
 * Every frame is seven fixed octets — a type, a channel, and the length of what
 * follows — then that many octets of payload, then the frame-end marker `0xCE`.
 * The marker is what makes a stream of frames resynchronisable in principle; in
 * practice one desynchronised byte is treated as fatal, which is why a bad one
 * is reported as such rather than skipped.
 *
 * The socket hands over arbitrary chunks, so decoding is a state machine that
 * accumulates: `feed()` adds bytes and `next()` returns a frame when a whole one
 * is buffered, or `null` when it needs more. The payload it returns is copied
 * out of the accumulation buffer, so a caller may hold it past the next `feed()`.
 */
import { ProtocolError } from "./errors.js";
import { ByteWriter } from "./wire.js";

/** The four frame kinds. */
export const FrameType = {
  method: 1,
  header: 2,
  body: 3,
  heartbeat: 8,
} as const;

export type FrameType = (typeof FrameType)[keyof typeof FrameType];

/** Whether a number is one of the four frame kinds. */
export function isFrameType(n: number): n is FrameType {
  return n === 1 || n === 2 || n === 3 || n === 8;
}

/** The octet every frame ends with. */
export const FRAME_END = 0xce;

/** Type, channel, and length: everything before a frame's payload. */
export const FRAME_HEADER_SIZE = 7;

/** A frame's fixed overhead: its header plus the frame-end octet. */
export const FRAME_OVERHEAD: number = FRAME_HEADER_SIZE + 1;

/**
 * The largest payload we accept unless the peer's `connection.tune` names a
 * smaller one. RabbitMQ's own default; the spec's minimum is 4096.
 */
export const DEFAULT_FRAME_MAX = 131072;

/** The eight octets a client opens a connection with: `AMQP` then `0 0 9 1`. */
export function encodeProtocolHeader(): Uint8Array {
  return new Uint8Array([0x41, 0x4d, 0x51, 0x50, 0x00, 0x00, 0x09, 0x01]);
}

/** One decoded frame. `payload` is a copy the caller owns. */
export interface Frame {
  type: FrameType;
  channel: number;
  payload: Uint8Array;
}

/** Frames a frame into bytes. */
export function encodeFrame(frame: Frame): Uint8Array {
  if (!isFrameType(frame.type)) {
    throw new ProtocolError(`unknown frame type ${frame.type}`);
  }
  if (frame.channel < 0 || frame.channel > 0xffff) {
    throw new ProtocolError(`a channel must be 0..65535; this one is ${frame.channel}`);
  }
  const w = new ByteWriter(FRAME_OVERHEAD + frame.payload.length);
  return w
    .u8(frame.type)
    .u16(frame.channel)
    .u32(frame.payload.length)
    .raw(frame.payload)
    .u8(FRAME_END)
    .finish();
}

/**
 * A resumable frame decoder. Feed it whatever the socket produced; take frames
 * out one at a time.
 */
export class FrameDecoder {
  #buf: Uint8Array;
  #view: DataView;
  #start = 0;
  #end = 0;
  #frameMax: number;

  constructor(options: { frameMax?: number } = {}) {
    this.#buf = new Uint8Array(0);
    this.#view = new DataView(this.#buf.buffer);
    this.#frameMax = options.frameMax ?? DEFAULT_FRAME_MAX;
  }

  /** Bytes buffered but not yet consumed by a frame. */
  get buffered(): number {
    return this.#end - this.#start;
  }

  /** Lowers or raises the accepted payload size, as `connection.tune` does. */
  set frameMax(bytes: number) {
    this.#frameMax = bytes;
  }

  /** Adds bytes to the buffer. */
  feed(bytes: Uint8Array): void {
    if (bytes.length === 0) return;
    this.#compact();
    this.#grow(bytes.length);
    this.#buf.set(bytes, this.#end);
    this.#end += bytes.length;
  }

  /** The next whole frame, or `null` if more bytes are needed. */
  next(): Frame | null {
    const remaining = this.#end - this.#start;
    if (remaining < FRAME_HEADER_SIZE) return null;

    const at = this.#start;
    const type = this.#view.getUint8(at);
    const channel = this.#view.getUint16(at + 1);
    const size = this.#view.getUint32(at + 3);

    if (!isFrameType(type)) {
      throw new ProtocolError(`unknown frame type ${type}`);
    }
    if (size > this.#frameMax) {
      throw new ProtocolError(
        `a frame of ${size} bytes exceeds the ${this.#frameMax}-byte maximum`,
      );
    }
    if (remaining < FRAME_HEADER_SIZE + size + 1) return null;

    const end = at + FRAME_HEADER_SIZE + size;
    if (this.#view.getUint8(end) !== FRAME_END) {
      throw new ProtocolError("a frame did not end with the 0xCE frame-end marker");
    }
    if (type === FrameType.heartbeat && (channel !== 0 || size !== 0)) {
      throw new ProtocolError("a heartbeat frame must be empty and on channel 0");
    }

    const payload = this.#buf.slice(at + FRAME_HEADER_SIZE, end);
    this.#start = end + 1;
    if (this.#start === this.#end) this.#start = this.#end = 0;
    return { type, channel, payload };
  }

  // Moves unconsumed bytes to the front so the buffer's tail is free to grow
  // into. Called before every append.
  #compact(): void {
    if (this.#start === 0) return;
    if (this.#start === this.#end) {
      this.#start = 0;
      this.#end = 0;
      return;
    }
    this.#buf.copyWithin(0, this.#start, this.#end);
    this.#end -= this.#start;
    this.#start = 0;
  }

  #grow(n: number): void {
    if (this.#end + n <= this.#buf.length) return;
    let size = Math.max(1024, this.#buf.length * 2);
    while (size < this.#end + n) size *= 2;
    const grown = new Uint8Array(size);
    grown.set(this.#buf.subarray(0, this.#end));
    this.#buf = grown;
    this.#view = new DataView(grown.buffer);
  }
}
