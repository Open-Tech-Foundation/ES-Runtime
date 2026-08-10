/**
 * Turning a RESP reply into what a caller sees — twice, for the two surfaces.
 *
 * `toValue` produces ordinary JavaScript, which is what the client API returns.
 * `rowsOf` presents the same replies as `runtime:db` rows. They agree on every
 * rule that matters, because a value that changed type depending on which door
 * it came through would be the sharpest edge in the package — so the row path
 * is `toValue` plus a flattening, not a second decoder.
 *
 * It hands the kit **records**: a RESP reply is already fully in memory and
 * already JavaScript, so encoding it into the shared byte layout only to have
 * `decodeBatch` take it apart again was work with no reader. That layout is
 * for backends that were handed bytes; this one never was.
 */
import { DbError, DbErrorCode, Rows, defineRecordShape } from "runtime:db";

import type { Reply } from "./resp.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

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

/** `JSON.stringify` refuses a bigint; a nested counter should not fail a query. */
function jsonSafe(_key: string, value: unknown): unknown {
  return typeof value === "bigint" ? value.toString() : value;
}

/**
 * One cell as an ordinary JavaScript value.
 *
 * A nested aggregate has nowhere to go in a flat row, so it becomes JSON text.
 * That is a real limitation and it is stated rather than hidden: the command
 * whose reply nests — `XRANGE`, `GEOPOS`, `EXEC` — is better read through the
 * command surface, which returns the structure itself.
 */
function cellValue(cell: Reply, options: DecodeOptions): unknown {
  switch (cell.kind) {
    case "null":
      return null;
    case "integer":
      return narrow(cell.value);
    case "boolean":
      return cell.value ? 1 : 0;
    case "double":
      return cell.value;
    case "bignumber":
      // Past what an i64 holds, so it stays exact as text rather than wrapping.
      return cell.value.toString();
    case "status":
      return cell.value;
    case "string":
      return options.binary ? cell.bytes : decoder.decode(cell.bytes);
    case "array":
    case "set":
    case "push":
    case "map":
      return JSON.stringify(toValue(cell, { binary: false }), jsonSafe);
    case "error":
      // Not returned as a value. An error that arrived inside an array is still
      // an error, and handing it back as a string would give the caller a row
      // that reads like data.
      throw nestedError(cell.value.prefix, cell.value.message);
  }
}

/**
 * How many rows are converted per batch.
 *
 * Rows rather than bytes, which is the natural unit once nothing is being
 * encoded — the kit's `maxBytes` budget describes a buffer this path does not
 * build. The number is a compromise between per-batch overhead and how much a
 * caller who stops early has already paid for.
 */
const ROWS_PER_BATCH = 1024;

/**
 * A reply as a result set.
 *
 * The whole reply is already in memory — RESP gives no way to know where one
 * ends without reading it — so there is nothing to stream and no cursor to
 * leave open, which is why `rows.exhausted` is always true and why a pooled
 * connection is free the moment the call returns.
 *
 * It is still converted **in batches**, and that is not vestigial. A ten
 * million element reply would otherwise become ten million arrays before the
 * caller saw the first one, and a caller who breaks out of the loop after three
 * rows would have paid for all of them. Batching is what keeps `LRANGE 0 -1`
 * followed by `break` cheap.
 */
export function rowsOf(reply: Reply, options: DecodeOptions): Rows {
  const { columns, rows: total } = shapeOf(reply);
  let at = 0;
  return new Rows(
    {
      exhausted: true,
      async next() {
        if (at >= total) return { records: [], done: true };
        const end = Math.min(total, at + ROWS_PER_BATCH);
        const records: unknown[][] = new Array(end - at);
        for (let slot = 0; at < end; at++, slot++) {
          records[slot] = cellsAt(reply, at).map((cell) => cellValue(cell, options));
        }
        return { records, done: at >= total };
      },
      async close() {},
    },
    defineRecordShape(columns),
  );
}

/** Decodes a bulk string the way `toValue` would, for the command helpers. */
export function asText(bytes: Uint8Array): string {
  return decoder.decode(bytes);
}
