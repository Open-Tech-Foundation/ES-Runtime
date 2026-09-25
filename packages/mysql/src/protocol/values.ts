/**
 * MySQL's binary protocol, both ways: result rows into `runtime:db`'s row
 * layout, and JavaScript values into `COM_STMT_EXECUTE` parameters.
 *
 * A binary row is a NULL bitmap followed by each non-NULL value in its own
 * encoding — fixed-width little-endian for numbers, a length prefix for
 * strings, a length-prefixed struct for dates and times. `runtime:db` wants
 * every row as `length(4) columns(2) [len(4) bytes]*`, so each row is
 * **transcoded** once into that layout as it comes off the socket, copying each
 * value's bytes as they are: nothing is decoded until a column is read, and the
 * decoders below read MySQL's own encoding out of the span.
 */
import { Payload, type Writer } from "./packets.js";

const DECODER = new TextDecoder();
const ENCODER = new TextEncoder();

/** `enum_field_types` — the types a column or a parameter can have. */
export const T = {
  DECIMAL: 0x00,
  TINY: 0x01,
  SHORT: 0x02,
  LONG: 0x03,
  FLOAT: 0x04,
  DOUBLE: 0x05,
  NULL: 0x06,
  TIMESTAMP: 0x07,
  LONGLONG: 0x08,
  INT24: 0x09,
  DATE: 0x0a,
  TIME: 0x0b,
  DATETIME: 0x0c,
  YEAR: 0x0d,
  VARCHAR: 0x0f,
  BIT: 0x10,
  VECTOR: 0xf2,
  JSON: 0xf5,
  NEWDECIMAL: 0xf6,
  ENUM: 0xf7,
  SET: 0xf8,
  TINY_BLOB: 0xf9,
  MEDIUM_BLOB: 0xfa,
  LONG_BLOB: 0xfb,
  BLOB: 0xfc,
  VAR_STRING: 0xfd,
  STRING: 0xfe,
  GEOMETRY: 0xff,
} as const;

/** Column flags this driver reads. */
const UNSIGNED_FLAG = 0x20;

/** The character set id MySQL uses for "no character set: these are bytes". */
const BINARY_CHARSET = 63;

/** What a `ColumnDefinition41` packet says, of the parts a decoder needs. */
export interface Column {
  name: string;
  type: number;
  flags: number;
  charset: number;
}

/** Reads a `ColumnDefinition41` payload. */
export function readColumn(payload: Uint8Array): Column {
  const p = new Payload(payload);
  p.skipLenenc(); // catalog, always "def"
  p.skipLenenc(); // schema
  p.skipLenenc(); // table alias
  p.skipLenenc(); // original table
  const name = p.lenencString();
  p.skipLenenc(); // original name
  p.count(); // length of the fixed fields, always 0x0c
  const charset = p.u16();
  p.u32(); // display length
  const type = p.u8();
  const flags = p.u16();
  return { name, type, flags, charset };
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/**
 * How each column's value is laid out in a binary row: a fixed width in bytes,
 * `-1` for a one-byte length then that many bytes (dates and times), or `0`
 * for a length-encoded string.
 */
export function widths(columns: readonly Column[]): Int8Array {
  const out = new Int8Array(columns.length);
  columns.forEach((column, i) => {
    out[i] = widthOf(column.type);
  });
  return out;
}

function widthOf(type: number): number {
  switch (type) {
    case T.TINY:
      return 1;
    case T.SHORT:
    case T.YEAR:
      return 2;
    case T.LONG:
    case T.INT24:
    case T.FLOAT:
      return 4;
    case T.LONGLONG:
    case T.DOUBLE:
      return 8;
    case T.DATE:
    case T.DATETIME:
    case T.TIMESTAMP:
    case T.TIME:
      return -1;
    default:
      return 0;
  }
}

/**
 * Rows gathered into one fresh buffer in the shared layout. Fresh per batch:
 * rows keep a reference to it, so reusing one would rewrite rows a caller
 * still holds.
 */
export class RowBatch {
  bytes: Uint8Array;
  view: DataView;
  size = 0;
  count = 0;

  constructor(capacity: number) {
    this.bytes = new Uint8Array(Math.max(capacity, 256));
    this.view = new DataView(this.bytes.buffer);
  }

  #reserve(n: number): void {
    if (this.size + n <= this.bytes.length) return;
    let capacity = this.bytes.length * 2;
    while (capacity < this.size + n) capacity *= 2;
    const grown = new Uint8Array(capacity);
    grown.set(this.bytes.subarray(0, this.size));
    this.bytes = grown;
    this.view = new DataView(grown.buffer);
  }

  /**
   * Transcodes one binary row — `0x00`, the NULL bitmap, the values — out of
   * `src` into this batch.
   *
   * The output can be no longer than the input plus four bytes per column (a
   * length replacing a length prefix of at least one byte), so the room is
   * reserved once per row rather than per value.
   */
  appendBinaryRow(
    src: Uint8Array,
    srcView: DataView,
    start: number,
    length: number,
    layout: Int8Array,
  ): void {
    const columns = layout.length;
    this.#reserve(length + 6 + columns * 4);
    const out = this.bytes;
    const view = this.view;
    const rowStart = this.size;
    let w = rowStart + 6;
    view.setInt16(rowStart + 4, columns);
    const bitmap = start + 1;
    let r = bitmap + ((columns + 9) >> 3);
    for (let c = 0; c < columns; c++) {
      const bit = c + 2;
      if ((src[bitmap + (bit >> 3)]! & (1 << (bit & 7))) !== 0) {
        view.setInt32(w, -1);
        w += 4;
        continue;
      }
      const width = layout[c]!;
      let size: number;
      if (width > 0) {
        size = width;
      } else if (width < 0) {
        size = src[r++]!;
      } else {
        const first = src[r++]!;
        if (first < 0xfb) {
          size = first;
        } else if (first === 0xfc) {
          size = srcView.getUint16(r, true);
          r += 2;
        } else if (first === 0xfd) {
          size = srcView.getUint16(r, true) | (src[r + 2]! << 16);
          r += 3;
        } else {
          size = srcView.getUint32(r, true) + srcView.getUint32(r + 4, true) * 0x1_0000_0000;
          r += 8;
        }
      }
      view.setInt32(w, size);
      w += 4;
      if (size <= 16) {
        for (let i = 0; i < size; i++) out[w + i] = src[r + i]!;
      } else {
        out.set(src.subarray(r, r + size), w);
      }
      w += size;
      r += size;
    }
    view.setInt32(rowStart, w - rowStart);
    this.size = w;
    this.count++;
  }

  get gathered(): Uint8Array {
    return this.bytes.subarray(0, this.size);
  }
}

// ---------------------------------------------------------------------------
// Decoders
// ---------------------------------------------------------------------------

export type Decoder = (bytes: Uint8Array, view: DataView, start: number, length: number) => unknown;

export interface DecodeOptions {
  temporal?: boolean;
}

const text: Decoder = (bytes, _view, start, length) =>
  DECODER.decode(bytes.subarray(start, start + length));

const raw: Decoder = (bytes, _view, start, length) => bytes.slice(start, start + length);

const decoders: Record<string, Decoder> = {
  i8: (_b, view, start) => view.getInt8(start),
  u8: (_b, view, start) => view.getUint8(start),
  i16: (_b, view, start) => view.getInt16(start, true),
  u16: (_b, view, start) => view.getUint16(start, true),
  i32: (_b, view, start) => view.getInt32(start, true),
  u32: (_b, view, start) => view.getUint32(start, true),
  // A bigint only where a number would lose the value: `row.id + 1` should work
  // for the ids people actually have, and stay exact for the ones they do not.
  i64: (_b, view, start) => {
    const high = view.getInt32(start + 4, true);
    if (high > -0x20_0000 && high < 0x20_0000) {
      return high * 0x1_0000_0000 + view.getUint32(start, true);
    }
    return view.getBigInt64(start, true);
  },
  u64: (_b, view, start) => {
    const high = view.getUint32(start + 4, true);
    if (high < 0x20_0000) return high * 0x1_0000_0000 + view.getUint32(start, true);
    return view.getBigUint64(start, true);
  },
  f32: (_b, view, start) => view.getFloat32(start, true),
  f64: (_b, view, start) => view.getFloat64(start, true),
  json: (bytes, view, start, length) => JSON.parse(text(bytes, view, start, length) as string),
};

/** A date or time struct's fields: absent ones are zero, which is what MySQL means. */
function dateParts(view: DataView, start: number, length: number) {
  return {
    year: length >= 4 ? view.getUint16(start, true) : 0,
    month: length >= 4 ? view.getUint8(start + 2) : 0,
    day: length >= 4 ? view.getUint8(start + 3) : 0,
    hour: length >= 7 ? view.getUint8(start + 4) : 0,
    minute: length >= 7 ? view.getUint8(start + 5) : 0,
    second: length >= 7 ? view.getUint8(start + 6) : 0,
    micro: length >= 11 ? view.getUint32(start + 7, true) : 0,
  };
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/**
 * The text MySQL itself would print. Used where no Temporal value can say it:
 * the zero date `0000-00-00`, which MySQL permits and no calendar has.
 */
function dateTimeText(view: DataView, start: number, length: number, withTime: boolean): string {
  const d = dateParts(view, start, length);
  const date = `${pad(d.year, 4)}-${pad(d.month)}-${pad(d.day)}`;
  if (!withTime) return date;
  const fraction = d.micro === 0 ? "" : `.${pad(d.micro, 6)}`;
  return `${date} ${pad(d.hour)}:${pad(d.minute)}:${pad(d.second)}${fraction}`;
}

/** Milliseconds since the epoch for a UTC wall time, for any year. */
function utcMillis(d: ReturnType<typeof dateParts>): number {
  const date = new Date(0);
  date.setUTCFullYear(d.year, d.month - 1, d.day);
  date.setUTCHours(d.hour, d.minute, d.second, Math.floor(d.micro / 1000));
  return date.getTime();
}

function isZeroDate(view: DataView, start: number, length: number): boolean {
  return length === 0 || (view.getUint16(start, true) === 0 && view.getUint8(start + 2) === 0);
}

const temporalDecoders: Record<string, Decoder> = {
  date: (_b, view, start, length) => {
    if (isZeroDate(view, start, length)) return dateTimeText(view, start, length, false);
    const d = dateParts(view, start, length);
    return new Temporal.PlainDate(d.year, d.month, d.day);
  },
  datetime: (_b, view, start, length) => {
    if (isZeroDate(view, start, length)) return dateTimeText(view, start, length, true);
    const d = dateParts(view, start, length);
    const ms = Math.floor(d.micro / 1000);
    return new Temporal.PlainDateTime(
      d.year,
      d.month,
      d.day,
      d.hour,
      d.minute,
      d.second,
      ms,
      d.micro - ms * 1000,
    );
  },
  // A TIMESTAMP is an instant: MySQL stores it in UTC and converts it to the
  // session's time zone on the way out. The session is set to UTC at connect,
  // so the wall time that arrives *is* UTC.
  timestamp: (_b, view, start, length) => {
    if (isZeroDate(view, start, length)) return dateTimeText(view, start, length, true);
    const d = dateParts(view, start, length);
    const ms = utcMillis(d);
    return Temporal.Instant.fromEpochNanoseconds(
      BigInt(ms) * 1_000_000n + BigInt(d.micro % 1000) * 1000n,
    );
  },
  // A TIME is a duration, not a time of day: it runs from -838:59:59 to
  // 838:59:59 and is what `TIMEDIFF` returns.
  time: (_b, view, start, length) => {
    if (length === 0) return new Temporal.Duration();
    const sign = view.getUint8(start) === 1 ? -1 : 1;
    const days = view.getUint32(start + 1, true);
    const micro = length >= 12 ? view.getUint32(start + 8, true) : 0;
    const ms = Math.floor(micro / 1000);
    return new Temporal.Duration(
      0,
      0,
      0,
      0,
      sign * (days * 24 + view.getUint8(start + 5)),
      sign * view.getUint8(start + 6),
      sign * view.getUint8(start + 7),
      sign * ms,
      sign * (micro - ms * 1000),
    );
  },
};

const legacyDecoders: Record<string, Decoder> = {
  date: (_b, view, start, length) => dateTimeText(view, start, length, false),
  datetime: (_b, view, start, length) =>
    isZeroDate(view, start, length)
      ? dateTimeText(view, start, length, true)
      : new Date(utcMillis(dateParts(view, start, length))),
  timestamp: (_b, view, start, length) =>
    isZeroDate(view, start, length)
      ? dateTimeText(view, start, length, true)
      : new Date(utcMillis(dateParts(view, start, length))),
  time: (_b, view, start, length) => {
    if (length === 0) return "00:00:00";
    const sign = view.getUint8(start) === 1 ? "-" : "";
    const hours = view.getUint32(start + 1, true) * 24 + view.getUint8(start + 5);
    const micro = length >= 12 ? view.getUint32(start + 8, true) : 0;
    const fraction = micro === 0 ? "" : `.${pad(micro, 6)}`;
    return `${sign}${pad(hours)}:${pad(view.getUint8(start + 6))}:${pad(view.getUint8(start + 7))}${fraction}`;
  },
};

/** The decoder for one column of a binary result. */
export function decoderFor(column: Column, options: DecodeOptions = {}): Decoder {
  const unsigned = (column.flags & UNSIGNED_FLAG) !== 0;
  const dates = options.temporal === false ? legacyDecoders : temporalDecoders;
  switch (column.type) {
    case T.TINY:
      return unsigned ? decoders.u8! : decoders.i8!;
    case T.SHORT:
      return unsigned ? decoders.u16! : decoders.i16!;
    case T.YEAR:
      return decoders.u16!;
    case T.LONG:
    case T.INT24:
      return unsigned ? decoders.u32! : decoders.i32!;
    case T.LONGLONG:
      return unsigned ? decoders.u64! : decoders.i64!;
    case T.FLOAT:
      return decoders.f32!;
    case T.DOUBLE:
      return decoders.f64!;
    case T.DATE:
      return dates.date!;
    case T.DATETIME:
      return dates.datetime!;
    case T.TIMESTAMP:
      return dates.timestamp!;
    case T.TIME:
      return dates.time!;
    case T.JSON:
      return decoders.json!;
    // Exact decimals stay text: a number would round them, and that is the one
    // thing a DECIMAL column exists to prevent.
    case T.DECIMAL:
    case T.NEWDECIMAL:
      return text;
    case T.BIT:
    case T.GEOMETRY:
    case T.VECTOR:
      return raw;
    default:
      // Every string and blob type: the character set says which it is.
      // `BINARY`, `VARBINARY` and the `BLOB`s carry the binary set; the
      // `CHAR`/`VARCHAR`/`TEXT`/`ENUM`/`SET` family carry a real one.
      return column.charset === BINARY_CHARSET ? raw : text;
  }
}

/**
 * What distinguishes one result's decoding from another's. Two results with
 * the same key can share a row class.
 */
export function shapeKey(columns: readonly Column[], options: DecodeOptions): string {
  let key = options.temporal === false ? "L" : "T";
  for (const c of columns) {
    key += `|${c.name}\u0000${c.type}.${c.flags & UNSIGNED_FLAG}.${c.charset === BINARY_CHARSET ? 1 : 0}`;
  }
  return key;
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

const I64_MIN = -(1n << 63n);
const I64_MAX = (1n << 63n) - 1n;
const U64_MAX = (1n << 64n) - 1n;

/**
 * Writes the parameter block of a `COM_STMT_EXECUTE`: the NULL bitmap, the
 * "types follow" flag, a type per parameter, then each non-NULL value.
 *
 * Types are sent with every execution rather than only the first. The server
 * would remember them, but only for as long as they do not change, and a
 * parameter that is a number in one call and a string in the next is ordinary
 * JavaScript.
 */
export function writeParams(w: Writer, params: readonly unknown[]): void {
  const n = params.length;
  if (n === 0) return;
  const bitmap = new Uint8Array((n + 7) >> 3);
  const types: number[] = [];
  const encoded: ((w: Writer) => void)[] = [];
  params.forEach((value, i) => {
    const param = encodeParam(value);
    if (param === null) {
      bitmap[i >> 3]! |= 1 << (i & 7);
      types.push(T.NULL, 0);
    } else {
      types.push(param.type, param.unsigned ? 0x80 : 0);
      encoded.push(param.write);
    }
  });
  w.bytes(bitmap).u8(1);
  for (const byte of types) w.u8(byte);
  for (const write of encoded) write(w);
}

interface Param {
  type: number;
  unsigned: boolean;
  write: (w: Writer) => void;
}

function encodeParam(value: unknown): Param | null {
  if (value === null || value === undefined) return null;
  switch (typeof value) {
    case "boolean":
      return { type: T.TINY, unsigned: false, write: (w) => w.u8(value ? 1 : 0) };
    case "number":
      if (Number.isSafeInteger(value)) {
        return { type: T.LONGLONG, unsigned: false, write: (w) => w.i64(BigInt(value)) };
      }
      if (!Number.isFinite(value)) {
        throw new TypeError(`MySQL has no representation for ${value}`);
      }
      return { type: T.DOUBLE, unsigned: false, write: (w) => w.f64(value) };
    case "bigint":
      if (value >= I64_MIN && value <= I64_MAX) {
        return { type: T.LONGLONG, unsigned: false, write: (w) => w.i64(value) };
      }
      if (value > I64_MAX && value <= U64_MAX) {
        return { type: T.LONGLONG, unsigned: true, write: (w) => w.u64(value) };
      }
      throw new RangeError(`${value} does not fit in a 64-bit integer`);
    case "string":
      return stringParam(value);
    default:
      break;
  }
  if (value instanceof Uint8Array) {
    return { type: T.BLOB, unsigned: false, write: (w) => w.lenencBytes(value) };
  }
  if (ArrayBuffer.isView(value)) {
    const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    return { type: T.BLOB, unsigned: false, write: (w) => w.lenencBytes(bytes) };
  }
  if (value instanceof ArrayBuffer) {
    const bytes = new Uint8Array(value);
    return { type: T.BLOB, unsigned: false, write: (w) => w.lenencBytes(bytes) };
  }
  if (value instanceof Date) {
    if (Number.isNaN(value.getTime()))
      throw new RangeError("an invalid Date cannot be a parameter");
    return datetimeParam(T.DATETIME, {
      year: value.getUTCFullYear(),
      month: value.getUTCMonth() + 1,
      day: value.getUTCDate(),
      hour: value.getUTCHours(),
      minute: value.getUTCMinutes(),
      second: value.getUTCSeconds(),
      micro: value.getUTCMilliseconds() * 1000,
    });
  }
  const temporal = temporalParam(value);
  if (temporal !== null) return temporal;
  // Arrays and plain objects are documents: MySQL's JSON type takes them as
  // text, and a `JSON` column validates what it is given.
  return stringParam(JSON.stringify(value));
}

function stringParam(value: string): Param {
  const bytes = ENCODER.encode(value);
  return { type: T.VAR_STRING, unsigned: false, write: (w) => w.lenencBytes(bytes) };
}

interface Parts {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
  micro: number;
}

function datetimeParam(type: number, d: Parts): Param {
  return {
    type,
    unsigned: false,
    write: (w) => {
      w.u8(11).u16(d.year).u8(d.month).u8(d.day).u8(d.hour).u8(d.minute).u8(d.second);
      w.u32(d.micro);
    },
  };
}

/**
 * Temporal values, each as the MySQL type that means the same thing. An
 * instant goes as its UTC wall time, which is what the session — set to UTC at
 * connect — reads it as.
 */
function temporalParam(value: unknown): Param | null {
  if (typeof Temporal === "undefined") return null;
  if (value instanceof Temporal.Instant) {
    return temporalParam(value.toZonedDateTimeISO("UTC").toPlainDateTime());
  }
  if (value instanceof Temporal.ZonedDateTime) {
    return temporalParam(value.toInstant());
  }
  if (value instanceof Temporal.PlainDateTime) {
    return datetimeParam(T.DATETIME, {
      year: value.year,
      month: value.month,
      day: value.day,
      hour: value.hour,
      minute: value.minute,
      second: value.second,
      micro: value.millisecond * 1000 + value.microsecond,
    });
  }
  if (value instanceof Temporal.PlainDate) {
    return {
      type: T.DATE,
      unsigned: false,
      write: (w) => w.u8(4).u16(value.year).u8(value.month).u8(value.day),
    };
  }
  if (value instanceof Temporal.PlainTime) {
    return timeParam(
      false,
      0,
      value.hour,
      value.minute,
      value.second,
      value.millisecond * 1000 + value.microsecond,
    );
  }
  if (value instanceof Temporal.Duration) {
    const total = value.round({ largestUnit: "hour", smallestUnit: "microsecond" });
    const negative = total.sign < 0;
    const hours = Math.abs(total.hours);
    return timeParam(
      negative,
      Math.floor(hours / 24),
      hours % 24,
      Math.abs(total.minutes),
      Math.abs(total.seconds),
      Math.abs(total.milliseconds) * 1000 + Math.abs(total.microseconds),
    );
  }
  return null;
}

function timeParam(
  negative: boolean,
  days: number,
  hour: number,
  minute: number,
  second: number,
  micro: number,
): Param {
  return {
    type: T.TIME,
    unsigned: false,
    write: (w) => {
      w.u8(12)
        .u8(negative ? 1 : 0)
        .u32(days)
        .u8(hour)
        .u8(minute)
        .u8(second)
        .u32(micro);
    },
  };
}
