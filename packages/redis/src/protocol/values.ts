/**
 * Turning a RESP reply into what a caller sees — twice, for the two surfaces.
 *
 * `toValue` produces ordinary JavaScript, which is what the client API returns.
 * `writeRows` transcodes into `runtime:db`'s shared row layout, which is what
 * `connection.query()` returns. They are separate functions and they agree on
 * every rule that matters, because a value that changed type depending on which
 * door it came through would be the sharpest edge in the package.
 *
 * The layout is the one D56 fixed for every backend: per row an `int32` length
 * counting itself, an `int16` column count, then per column an `int32` length
 * (`-1` for NULL) and that many bytes. Redis replies carry a type per value
 * rather than per column — an array need not be homogeneous — so each cell is
 * written in the kit's **tagged** encoding and read back by its dynamic
 * decoder, exactly as the embedded SQLite backend's values are.
 */
import { ByteWriter, DbError, DbErrorCode } from "runtime:db";

import type { Reply } from "./resp.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

// The kit's per-value tags. Shared with the host and with every backend that
// produces rows in this layout.
const TAG_INTEGER = 1;
const TAG_REAL = 2;
const TAG_TEXT = 3;
const TAG_BLOB = 4;

const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const MIN_SAFE = -MAX_SAFE;

export interface DecodeOptions {
  /**
   * Hand bulk strings back as `Uint8Array` rather than decoding them as UTF-8.
   *
   * Redis values are binary-safe and text is only the common case. The default
   * is text because that is what almost every key holds and a `Uint8Array` per
   * value would make the ordinary path hostile; `binary` is for the callers who
   * store something else and would otherwise get mojibake.
   */
  binary?: boolean;
}

/**
 * A Redis integer as JavaScript.
 *
 * A `number` where one holds the value exactly and a `bigint` where it does
 * not. This is the kit's own rule — `decodeDynamic` applies it to every backend
 * — and it matters here because Redis integers are signed 64-bit: `INCRBY` past
 * 2^53 is a thing an application does, and answering `number` always would
 * round it silently while answering `bigint` always would make `row.n + 1`
 * throw on the ninety-nine percent of counters that fit.
 */
function narrow(value: bigint): number | bigint {
  return value >= MIN_SAFE && value <= MAX_SAFE ? Number(value) : value;
}

/** Raised for an error reply nested inside another reply. */
function nestedError(prefix: string, message: string): DbError {
  return new DbError(message, { code: DbErrorCode.Backend, backendCode: prefix });
}

/**
 * A reply as ordinary JavaScript.
 *
 * Maps become plain objects, because `HGETALL` is the reason maps exist and an
 * object is what a caller wants from one. Keys are stringified: RESP3 permits
 * any type there and Redis has never sent a non-string.
 */
export function toValue(reply: Reply, options: DecodeOptions = {}): unknown {
  switch (reply.kind) {
    case "null":
      return null;
    case "status":
      return reply.value;
    case "string":
      return options.binary ? reply.bytes : reply.value;
    case "integer":
      return narrow(reply.value);
    case "bignumber":
      // Past 64 bits by definition, so there is no narrowing to do.
      return reply.value;
    case "double":
      return reply.value;
    case "boolean":
      return reply.value;
    case "array":
    case "set":
    case "push":
      return reply.value.map((item) => toValue(item, options));
    case "map": {
      const out: Record<string, unknown> = {};
      for (const [key, value] of reply.value) {
        out[String(toValue(key, { binary: false }))] = toValue(value, options);
      }
      return out;
    }
    case "error":
      throw nestedError(reply.value.prefix, reply.value.message);
  }
}

// ---------------------------------------------------------------------------
// The row layout
// ---------------------------------------------------------------------------

/**
 * What shape of result a reply becomes.
 *
 * Decided by the reply's own type and nothing else — no table of which command
 * returns what. A map is two columns because a map is pairs; an aggregate is
 * one column and many rows; anything else is one column and one row. A command
 * table would be a second thing to keep correct as Redis grows, and it would be
 * wrong for `EVAL`, whose shape is whatever the script returned.
 */
export function shapeOf(reply: Reply): { columns: { name: string; declType: string | null }[]; rows: number } {
  switch (reply.kind) {
    case "null":
      // Distinct from an empty array, and both are zero rows. `GET` on a
      // missing key answers no row, which is what `rows.first() === null`
      // already means everywhere else.
      return { columns: [VALUE], rows: 0 };
    case "map":
      return { columns: [FIELD, VALUE], rows: reply.value.length };
    case "array":
    case "set":
    case "push":
      return { columns: [VALUE], rows: reply.value.length };
    default:
      return { columns: [VALUE], rows: 1 };
  }
}

const VALUE = { name: "value", declType: null };
const FIELD = { name: "field", declType: null };

/** The cells of row `index`, in the shape `shapeOf` reported. */
function cellsAt(reply: Reply, index: number): Reply[] {
  switch (reply.kind) {
    case "map": {
      const pair = reply.value[index]!;
      return [pair[0], pair[1]];
    }
    case "array":
    case "set":
    case "push":
      return [reply.value[index]!];
    default:
      return [reply];
  }
}

/**
 * Writes one cell in the kit's tagged encoding.
 *
 * A nested aggregate has nowhere to go in a flat row, so it is written as JSON
 * text. That is a real limitation and it is stated rather than hidden: the
 * command whose reply nests — `XRANGE`, `GEOPOS`, `EXEC` — is better read
 * through the client API, which returns the structure itself.
 */
function writeCell(w: ByteWriter, cell: Reply, options: DecodeOptions): void {
  switch (cell.kind) {
    case "null":
      // A length of -1 is the whole of NULL: no tag, no payload.
      w.i32(-1);
      return;
    case "integer":
      w.i32(9).u8(TAG_INTEGER).i64(cell.value);
      return;
    case "boolean":
      w.i32(9).u8(TAG_INTEGER).i64(cell.value ? 1 : 0);
      return;
    case "double":
      w.i32(9).u8(TAG_REAL).f64(cell.value);
      return;
    case "bignumber":
      // Past what `i64` holds, so it stays exact as text rather than wrapping.
      writeText(w, cell.value.toString());
      return;
    case "status":
      writeText(w, cell.value);
      return;
    case "string":
      if (options.binary) {
        w.i32(cell.bytes.length + 1).u8(TAG_BLOB).bytes(cell.bytes);
      } else {
        w.i32(cell.bytes.length + 1).u8(TAG_TEXT).bytes(cell.bytes);
      }
      return;
    case "array":
    case "set":
    case "push":
    case "map":
      writeText(w, JSON.stringify(toValue(cell, { binary: false }), jsonSafe));
      return;
    case "error":
      // Not written as a value. An error that arrived inside an array is still
      // an error, and turning it into a string cell would hand the caller a row
      // that reads like data.
      throw nestedError(cell.value.prefix, cell.value.message);
  }
}

function writeText(w: ByteWriter, text: string): void {
  const bytes = encoder.encode(text);
  w.i32(bytes.length + 1).u8(TAG_TEXT).bytes(bytes);
}

/** `JSON.stringify` refuses a bigint; a nested counter should not fail a query. */
function jsonSafe(_key: string, value: unknown): unknown {
  return typeof value === "bigint" ? value.toString() : value;
}

/**
 * Encodes rows `[from, …)` of `reply` into one batch, stopping at `maxBytes`.
 *
 * A whole reply is already in memory — RESP gives no way to know where a reply
 * ends without reading it — so batching buys nothing on the wire. What it buys
 * is that `decodeBatch` is not asked to build ten million row objects at once,
 * and that a caller who breaks out of the loop after three rows never pays for
 * the rest.
 */
export function writeRows(
  reply: Reply,
  from: number,
  total: number,
  maxBytes: number,
  options: DecodeOptions,
): { bytes: Uint8Array; rows: number; done: boolean } {
  const w = new ByteWriter(Math.min(maxBytes, 8192));
  let written = 0;
  let at = from;
  while (at < total) {
    const cells = cellsAt(reply, at);
    const start = w.beginLength();
    w.i16(cells.length);
    for (const cell of cells) writeCell(w, cell, options);
    // Inclusive of the length field itself, which is how the decoder walks from
    // one row to the next.
    w.endLength(start, { inclusive: true });
    at++;
    written++;
    if (w.length >= maxBytes) break;
  }
  return { bytes: w.finish(), rows: written, done: at >= total };
}

/** Decodes a bulk string the way `toValue` would, for the command helpers. */
export function asText(bytes: Uint8Array): string {
  return decoder.decode(bytes);
}
