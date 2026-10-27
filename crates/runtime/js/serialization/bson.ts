// BSON (DECISIONS D187). `encode` and `decode` are engine builtins that walk
// live values; this file defines the BSON types they recognize, and hands the
// builtins those types' prototypes on every call (see crates/engine/src/bson.rs
// for the payload each one carries).

type Ops = Record<string, (...a: any[]) => any>;
const g = globalThis as unknown as {
  __ops: Ops;
  __bson_encode: (value: unknown, types: readonly unknown[]) => Uint8Array;
  __bson_decode: (
    bytes: Uint8Array | ArrayBuffer,
    types: readonly unknown[],
  ) => Record<string, unknown>;
};
// Captured now: the globals are hidden, not locked (harden.js).
const { bson_validate, bson_decimal128_parse, bson_decimal128_string } = g.__ops;
const bsonEncode = g.__bson_encode;
const bsonDecode = g.__bson_decode;

/** Where an ObjectId, Decimal128, UUID or Timestamp keeps its value. */
const DATA = Symbol("bson.value");
const INSPECT = Symbol.for("nodejs.util.inspect.custom");
const HEX = /^[0-9a-f]+$/i;

function hex(bytes: Uint8Array): string {
  let out = "";
  for (const b of bytes) out += (b < 16 ? "0" : "") + b.toString(16);
  return out;
}

function unhex(text: string): Uint8Array {
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(text.slice(i * 2, i * 2 + 2), 16);
  return out;
}

// ---- ObjectId ---------------------------------------------------------------

// The 5 random bytes are this process's for its lifetime, and the counter starts
// at a random value, as the ObjectId specification requires.
let processUnique: string | null = null;
let counter = -1;

function generateObjectId(seconds: number): string {
  if (processUnique === null) {
    const random = crypto.getRandomValues(new Uint8Array(8));
    processUnique = hex(random.subarray(0, 5));
    counter = ((random[5] << 16) | (random[6] << 8) | random[7]) >>> 0;
  }
  counter = (counter + 1) % 0x1000000;
  return (
    (seconds >>> 0).toString(16).padStart(8, "0") +
    processUnique +
    counter.toString(16).padStart(6, "0")
  );
}

/** A 12-byte MongoDB object id. */
export class ObjectId {
  declare [DATA]: string;

  /** A new id, or the one given as 24 hex digits, 12 bytes, or another ObjectId. */
  constructor(id?: string | Uint8Array | ObjectId) {
    let value: string;
    if (id === undefined) value = generateObjectId(Math.floor(Date.now() / 1000));
    else if (id instanceof ObjectId) value = id[DATA];
    else if (typeof id === "string" && id.length === 24 && HEX.test(id)) value = id.toLowerCase();
    else if (id instanceof Uint8Array && id.length === 12) value = hex(id);
    else throw new TypeError("an ObjectId is 24 hex digits or 12 bytes");
    this[DATA] = value;
  }

  /** Whether `id` is something an ObjectId can be made from. */
  static isValid(id: unknown): boolean {
    return (
      id instanceof ObjectId ||
      (typeof id === "string" && id.length === 24 && HEX.test(id)) ||
      (id instanceof Uint8Array && id.length === 12)
    );
  }

  static createFromHexString(hexString: string): ObjectId {
    return new ObjectId(hexString);
  }

  /** An id whose timestamp is `seconds` and whose other bytes are zero, for range queries. */
  static createFromTime(seconds: number): ObjectId {
    return new ObjectId((seconds >>> 0).toString(16).padStart(8, "0") + "0".repeat(16));
  }

  /** The 12 bytes. */
  get id(): Uint8Array {
    return unhex(this[DATA]);
  }

  /** When the id was made, to the second. */
  getTimestamp(): Date {
    return new Date(parseInt(this[DATA].slice(0, 8), 16) * 1000);
  }

  equals(other: ObjectId | string | null | undefined): boolean {
    if (other instanceof ObjectId) return other[DATA] === this[DATA];
    return typeof other === "string" && other.toLowerCase() === this[DATA];
  }

  toHexString(): string {
    return this[DATA];
  }

  toString(): string {
    return this[DATA];
  }

  toJSON(): string {
    return this[DATA];
  }

  [INSPECT](): string {
    return `ObjectId("${this[DATA]}")`;
  }
}

// ---- Decimal128 -------------------------------------------------------------

/** A 128-bit decimal floating-point number, exactly as the server stores it. */
export class Decimal128 {
  declare [DATA]: string;

  /** From decimal text (exactly, or a `SyntaxError`) or the 16 stored bytes. */
  constructor(value: string | Uint8Array) {
    if (typeof value === "string") this[DATA] = bson_decimal128_parse(value);
    else if (value instanceof Uint8Array && value.length === 16) this[DATA] = hex(value);
    else throw new TypeError("a Decimal128 is made from a string or 16 bytes");
  }

  static fromString(text: string): Decimal128 {
    return new Decimal128(text);
  }

  /** The 16 bytes, as stored. */
  get bytes(): Uint8Array {
    return unhex(this[DATA]);
  }

  toString(): string {
    return bson_decimal128_string(this[DATA]);
  }

  toJSON(): { $numberDecimal: string } {
    return { $numberDecimal: this.toString() };
  }

  [INSPECT](): string {
    return `Decimal128("${this.toString()}")`;
  }
}

// ---- UUID -------------------------------------------------------------------

/** A UUID, stored as binary subtype 4. */
export class UUID {
  declare [DATA]: string;

  /** A new random (version 4) UUID, or the one given as text (dashes optional) or 16 bytes. */
  constructor(value?: string | Uint8Array | UUID) {
    let data: string;
    if (value === undefined) data = crypto.randomUUID().replaceAll("-", "");
    else if (value instanceof UUID) data = value[DATA];
    else if (typeof value === "string" && UUID.isValid(value))
      data = value.replaceAll("-", "").toLowerCase();
    else if (value instanceof Uint8Array && value.length === 16) data = hex(value);
    else throw new TypeError("a UUID is 32 hex digits, optionally dashed, or 16 bytes");
    this[DATA] = data;
  }

  static isValid(value: unknown): boolean {
    if (value instanceof UUID) return true;
    if (value instanceof Uint8Array) return value.length === 16;
    return (
      typeof value === "string" &&
      (/^[0-9a-f]{32}$/i.test(value) ||
        /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value))
    );
  }

  /** The 16 bytes. */
  get bytes(): Uint8Array {
    return unhex(this[DATA]);
  }

  equals(other: UUID | string | null | undefined): boolean {
    if (other instanceof UUID) return other[DATA] === this[DATA];
    return (
      typeof other === "string" &&
      UUID.isValid(other) &&
      other.replaceAll("-", "").toLowerCase() === this[DATA]
    );
  }

  /** The text form, dashed unless `includeDashes` is false. */
  toHexString(includeDashes = true): string {
    const d = this[DATA];
    if (!includeDashes) return d;
    return `${d.slice(0, 8)}-${d.slice(8, 12)}-${d.slice(12, 16)}-${d.slice(16, 20)}-${d.slice(20)}`;
  }

  toString(): string {
    return this.toHexString();
  }

  toJSON(): string {
    return this.toHexString();
  }

  [INSPECT](): string {
    return `UUID("${this.toHexString()}")`;
  }
}

// ---- Timestamp --------------------------------------------------------------

/** A server timestamp: seconds (`t`) and an ordinal within the second (`i`). */
export class Timestamp {
  declare [DATA]: bigint;

  constructor(value: { t: number; i: number } | bigint) {
    if (typeof value === "bigint") {
      if (value < 0n || value > 0xffffffffffffffffn) throw new RangeError("a Timestamp is 64 bits");
      this[DATA] = value;
      return;
    }
    const { t, i } = value ?? ({} as { t: number; i: number });
    const u32 = (n: unknown) =>
      Number.isInteger(n) && (n as number) >= 0 && (n as number) <= 0xffffffff;
    if (!u32(t) || !u32(i))
      throw new RangeError("a Timestamp's t and i are integers from 0 to 2^32 - 1");
    this[DATA] = (BigInt(t) << 32n) | BigInt(i);
  }

  get t(): number {
    return Number(this[DATA] >> 32n);
  }

  get i(): number {
    return Number(this[DATA] & 0xffffffffn);
  }

  toJSON(): { $timestamp: { t: number; i: number } } {
    return { $timestamp: { t: this.t, i: this.i } };
  }

  [INSPECT](): string {
    return `Timestamp({ t: ${this.t}, i: ${this.i} })`;
  }
}

// ---- Binary -----------------------------------------------------------------

const VECTOR = { int8: 0x03, float32: 0x27, packedBit: 0x10 } as const;

/** Binary data of any subtype. A plain `Uint8Array` is subtype 0 and needs no wrapper. */
export class Binary {
  static readonly SUBTYPE_DEFAULT = 0;
  static readonly SUBTYPE_FUNCTION = 1;
  static readonly SUBTYPE_BYTE_ARRAY = 2;
  static readonly SUBTYPE_UUID_OLD = 3;
  static readonly SUBTYPE_UUID = 4;
  static readonly SUBTYPE_MD5 = 5;
  static readonly SUBTYPE_ENCRYPTED = 6;
  static readonly SUBTYPE_COLUMN = 7;
  static readonly SUBTYPE_SENSITIVE = 8;
  static readonly SUBTYPE_VECTOR = 9;
  static readonly SUBTYPE_USER_DEFINED = 128;

  buffer: Uint8Array;
  subType: number;

  constructor(buffer: Uint8Array = new Uint8Array(0), subType = 0) {
    if (!(buffer instanceof Uint8Array)) throw new TypeError("a Binary's buffer is a Uint8Array");
    if (!Number.isInteger(subType) || subType < 0 || subType > 255) {
      throw new RangeError("a Binary's subType is an integer from 0 to 255");
    }
    this.buffer = buffer;
    this.subType = subType;
  }

  /** A float32 vector (subtype 9), as Atlas Vector Search stores embeddings. */
  static fromFloat32Array(values: Float32Array | readonly number[]): Binary {
    if (!(values instanceof Float32Array)) values = Float32Array.from(numbers(values));
    const out = new Uint8Array(2 + values.length * 4);
    out[0] = VECTOR.float32;
    const view = new DataView(out.buffer);
    for (let i = 0; i < values.length; i++) view.setFloat32(2 + i * 4, values[i], true);
    return new Binary(out, Binary.SUBTYPE_VECTOR);
  }

  /** An int8 vector (subtype 9). */
  static fromInt8Array(values: Int8Array | readonly number[]): Binary {
    if (!(values instanceof Int8Array)) values = Int8Array.from(integers(values, -128, 127));
    const out = new Uint8Array(2 + values.length);
    out[0] = VECTOR.int8;
    out.set(new Uint8Array(values.buffer, values.byteOffset, values.length), 2);
    return new Binary(out, Binary.SUBTYPE_VECTOR);
  }

  /**
   * A packed-bit vector (subtype 9): `bits` holds 8 dimensions a byte, and the
   * last `padding` bits of the last byte are not dimensions, so they must be 0.
   */
  static fromPackedBits(bits: Uint8Array | readonly number[], padding = 0): Binary {
    if (!(bits instanceof Uint8Array)) bits = Uint8Array.from(integers(bits, 0, 255));
    checkPadding(bits, padding);
    const out = new Uint8Array(2 + bits.length);
    out[0] = VECTOR.packedBit;
    out[1] = padding;
    out.set(bits, 2);
    return new Binary(out, Binary.SUBTYPE_VECTOR);
  }

  toFloat32Array(): Float32Array {
    const data = vectorData(this, VECTOR.float32);
    if (data.length % 4 !== 0)
      throw new RangeError("a float32 vector's data is not a whole number of floats");
    const view = new DataView(data.buffer, data.byteOffset, data.length);
    const out = new Float32Array(data.length / 4);
    for (let i = 0; i < out.length; i++) out[i] = view.getFloat32(i * 4, true);
    return out;
  }

  toInt8Array(): Int8Array {
    const data = vectorData(this, VECTOR.int8);
    return new Int8Array(data.slice().buffer);
  }

  /** The packed bytes; `this.buffer[1]` is the padding. */
  toPackedBits(): Uint8Array {
    const data = vectorData(this, VECTOR.packedBit);
    checkPadding(data, this.buffer[1]);
    return data.slice();
  }

  toJSON(): { $binary: { base64: string; subType: string } } {
    let text = "";
    for (const b of this.buffer) text += String.fromCharCode(b);
    return { $binary: { base64: btoa(text), subType: this.subType.toString(16).padStart(2, "0") } };
  }

  [INSPECT](): string {
    return `Binary(${this.buffer.length} bytes, subType ${this.subType})`;
  }
}

// The data of a vector of `dtype`, after its two header bytes. A function
// rather than a private method: a decoded Binary is built by the engine with
// this class's prototype but never runs the constructor, so it has no private
// brand. The BSON classes keep no private state for the same reason.
function vectorData(binary: Binary, dtype: number): Uint8Array {
  const { buffer } = binary;
  if (binary.subType !== Binary.SUBTYPE_VECTOR || buffer.length < 2) {
    throw new TypeError("this Binary is not a vector");
  }
  if (buffer[0] !== dtype) throw new TypeError("this vector holds another element type");
  if (dtype !== VECTOR.packedBit && buffer[1] !== 0) {
    throw new RangeError("only a packed-bit vector has padding");
  }
  return buffer.subarray(2);
}

// A typed array wraps an out-of-range number silently (`Int8Array.of(128)` is
// -128), so values given as numbers are checked before they are converted.
function numbers(values: readonly number[]): readonly number[] {
  for (const v of values) {
    if (typeof v !== "number") throw new TypeError(`${String(v)} is not a number`);
  }
  return values;
}

function integers(values: readonly number[], min: number, max: number): readonly number[] {
  for (const v of values) {
    if (!Number.isInteger(v) || v < min || v > max) {
      throw new RangeError(`${v} is not an integer from ${min} to ${max}`);
    }
  }
  return values;
}

function checkPadding(bits: Uint8Array, padding: number): void {
  if (!Number.isInteger(padding) || padding < 0 || padding > 7) {
    throw new RangeError("a packed-bit vector's padding is 0 to 7");
  }
  if (padding > 0 && bits.length === 0)
    throw new RangeError("an empty packed-bit vector has no padding");
  if (padding > 0 && (bits[bits.length - 1] & ((1 << padding) - 1)) !== 0) {
    throw new RangeError("a packed-bit vector's padding bits must be 0");
  }
}

// ---- the rest ---------------------------------------------------------------

/** JavaScript code, with an optional scope (deprecated by the server, still readable). */
export class Code {
  code: string;
  scope: Record<string, unknown> | null;

  constructor(
    code: string | ((...args: unknown[]) => unknown),
    scope: Record<string, unknown> | null = null,
  ) {
    this.code = String(code);
    this.scope = scope;
  }

  toJSON(): { code: string; scope?: Record<string, unknown> } {
    return this.scope === null ? { code: this.code } : { code: this.code, scope: this.scope };
  }
}

/**
 * A regular expression exactly as the server stores it. Reading one gives this
 * rather than a `RegExp`: a server pattern may not compile in JavaScript, and
 * the `l` and `x` options have no JavaScript flag. Writing a `RegExp` also works.
 */
export class BSONRegExp {
  pattern: string;
  options: string;

  constructor(pattern: string, options = "") {
    if (pattern.includes("\0") || options.includes("\0")) {
      throw new TypeError("a regular expression cannot contain a NUL character");
    }
    if (!/^[ilmsux]*$/.test(options))
      throw new SyntaxError(`unknown regular expression options "${options}"`);
    this.pattern = pattern;
    this.options = [...options].sort().join("");
  }

  /** The JavaScript `RegExp`, when the options have JavaScript flags. */
  toRegExp(): RegExp {
    if (/[lx]/.test(this.options)) {
      throw new SyntaxError(`the "${this.options}" options have no JavaScript equivalent`);
    }
    return new RegExp(this.pattern, this.options);
  }

  toJSON(): { $regularExpression: { pattern: string; options: string } } {
    return { $regularExpression: { pattern: this.pattern, options: this.options } };
  }
}

/** Sorts below every other value. */
export class MinKey {
  toJSON(): { $minKey: 1 } {
    return { $minKey: 1 };
  }
}

/** Sorts above every other value. */
export class MaxKey {
  toJSON(): { $maxKey: 1 } {
    return { $maxKey: 1 };
  }
}

/**
 * A number that encodes as a double even when it is whole. A plain number
 * encodes as int32 when it is a whole number in that range, as in the official
 * driver; reading always gives a plain number.
 */
export class Double {
  value: number;

  constructor(value: number) {
    if (typeof value !== "number") throw new TypeError("a Double is made from a number");
    this.value = value;
  }

  valueOf(): number {
    return this.value;
  }

  toJSON(): number {
    return this.value;
  }
}

// The order crates/engine/src/bson.rs reads them in.
const TYPES = Object.freeze([
  DATA,
  ObjectId.prototype,
  Decimal128.prototype,
  UUID.prototype,
  Timestamp.prototype,
  Binary.prototype,
  Code.prototype,
  BSONRegExp.prototype,
  MinKey.prototype,
  MaxKey.prototype,
  Double.prototype,
]);

export const BSON = {
  /** A document (an object or a `Map`) as BSON bytes. */
  encode: (document: object): Uint8Array => bsonEncode(document, TYPES),
  /** BSON bytes as a document. Malformed bytes are a `SyntaxError`. */
  decode: (bytes: Uint8Array | ArrayBuffer): Record<string, unknown> => bsonDecode(bytes, TYPES),
  validate: (bytes: Uint8Array, options: { detailed?: boolean } = {}) => {
    const result = bson_validate(bytes);
    if (result === true) return options.detailed ? { valid: true } : true;
    return options.detailed ? { valid: false, error: result } : false;
  },
  ObjectId,
  Decimal128,
  UUID,
  Timestamp,
  Binary,
  Code,
  BSONRegExp,
  MinKey,
  MaxKey,
  Double,
};
