/**
 * Turning JavaScript values into parameter text, and column text back into
 * JavaScript values.
 *
 * Text format both ways for this release. Binary results are the larger win —
 * an `int8` arrives as eight bytes instead of a string to parse — but they
 * require the extended protocol to name a format per column, which is a change
 * to `Bind` and to the decoder table together, and it is worth landing on top
 * of something that already works.
 */

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const MIN_SAFE = -MAX_SAFE;

/** Type OIDs this driver decodes specially. Everything else stays a string. */
export const OID = {
  bool: 16,
  bytea: 17,
  int8: 20,
  int2: 21,
  int4: 23,
  text: 25,
  json: 114,
  float4: 700,
  float8: 701,
  varchar: 1043,
  date: 1082,
  time: 1083,
  timestamp: 1114,
  timestamptz: 1184,
  numeric: 1700,
  uuid: 2950,
  jsonb: 3802,
} as const;

export type Decoder = (
  bytes: Uint8Array,
  view: DataView,
  start: number,
  length: number,
) => unknown;

function text(bytes: Uint8Array, _view: DataView, start: number, length: number): string {
  return DECODER.decode(bytes.subarray(start, start + length));
}

/**
 * Decoders written against **text**, not byte spans.
 *
 * Array elements arrive as substrings of the array literal rather than as spans
 * of the row buffer, so the per-type knowledge has to be reachable from a
 * string — otherwise every element would need its own encode/decode round trip
 * to reuse it. The span decoders below are derived from these.
 */
const fromText: Record<number, (text: string) => unknown> = {
  [OID.bool]: (t) => t === "t",
  [OID.int2]: Number,
  [OID.int4]: Number,
  [OID.int8]: (t) => {
    // A bigint only where a number would lose the value: `row.id + 1` should
    // work for the ids people actually have, and stay exact for the ones they
    // do not.
    const value = BigInt(t);
    return value >= MIN_SAFE && value <= MAX_SAFE ? Number(value) : value;
  },
  [OID.float4]: Number,
  [OID.float8]: Number,
  // `numeric` stays a string. It is arbitrary precision by definition, and a
  // double is the one representation guaranteed to lose it — a column chosen
  // for exactness should not be rounded on the way out.
  [OID.numeric]: (t) => t,
  [OID.json]: JSON.parse,
  [OID.jsonb]: JSON.parse,
  [OID.timestamptz]: (t) => new Date(t),
  [OID.timestamp]: (t) => new Date(`${t}Z`),
  [OID.bytea]: (t) => {
    // `\x` hex is the modern output format; the legacy escape format is not
    // produced by any supported server version.
    if (!t.startsWith("\\x")) return ENCODER.encode(t);
    const out = new Uint8Array((t.length - 2) / 2);
    for (let i = 0; i < out.length; i++) {
      out[i] = Number.parseInt(t.substr(2 + i * 2, 2), 16);
    }
    return out;
  },
};

/**
 * Array type OIDs, mapped to the type of their elements.
 *
 * PostgreSQL gives every type an array type, and the wire says nothing about
 * the relationship — a column of `int4[]` reports OID 1007 and that is all, with
 * no hint that it is an array or of what. So the pairs are listed.
 *
 * An array of a type **not** listed comes back as its raw literal string
 * (`"{\"(1,2)\"}"`), not as an array of strings: without knowing the column is
 * an array there is nothing to tell it from a text column that happens to
 * contain braces, and guessing would corrupt the latter. Learning the rest
 * would mean querying `pg_type` at connect time, which is a round trip on every
 * connection to serve the types almost nobody selects.
 */
const ARRAY_ELEMENT: Record<number, number> = {
  199: OID.json,
  1000: OID.bool,
  1001: OID.bytea,
  1005: OID.int2,
  1007: OID.int4,
  1009: OID.text,
  1015: OID.varchar,
  1016: OID.int8,
  1021: OID.float4,
  1022: OID.float8,
  1115: OID.timestamp,
  1182: OID.date,
  1185: OID.timestamptz,
  1231: OID.numeric,
  2951: OID.uuid,
  3807: OID.jsonb,
};

/**
 * Parses PostgreSQL's array literal: `{1,2,3}`, `{"a,b",NULL}`, `{{1,2},{3,4}}`.
 *
 * Written out rather than reached for with a regular expression, because the
 * format nests and quotes: an element may contain the delimiter, a brace, or a
 * quote, and only tracking the quoting tells them apart. An **unquoted** `NULL`
 * is the null element; a quoted `"NULL"` is the four-character string, and
 * conflating them would turn data into absence.
 */
export function parseArray(literal: string, element: (text: string) => unknown): unknown[] {
  let at = 0;
  // A literal may carry an explicit dimension prefix (`[1:3]={…}`) when its
  // lower bound is not 1. The bounds change no value, so they are skipped.
  const equals = literal.indexOf("=");
  if (literal.startsWith("[") && equals !== -1) at = equals + 1;

  function parseList(): unknown[] {
    at++; // past '{'
    const out: unknown[] = [];
    if (literal[at] === "}") {
      at++;
      return out;
    }
    for (;;) {
      out.push(parseItem());
      const ch = literal[at];
      if (ch === ",") {
        at++;
        continue;
      }
      at++; // past '}'
      return out;
    }
  }

  function parseItem(): unknown {
    if (literal[at] === "{") return parseList();
    if (literal[at] === '"') {
      at++;
      let text = "";
      while (at < literal.length && literal[at] !== '"') {
        text += literal[at] === "\\" ? literal[++at] : literal[at];
        at++;
      }
      at++; // past the closing quote
      return element(text);
    }
    const start = at;
    while (at < literal.length && literal[at] !== "," && literal[at] !== "}") at++;
    const raw = literal.slice(start, at);
    return raw === "NULL" ? null : element(raw);
  }

  return literal[at] === "{" ? parseList() : [];
}

/** PostgreSQL counts time from 2000-01-01, not 1970. */
const PG_EPOCH_MS = 946_684_800_000;

/**
 * Decoders for the binary wire format, by OID.
 *
 * Only the types where binary is *simpler and cheaper* than text. An `int8`
 * arrives as eight bytes to read rather than up to nineteen digits to parse; a
 * `float8` is the double itself rather than a decimal rendering of it to
 * reconstruct.
 *
 * Three families deliberately stay on text. `numeric` is a base-10000 digit
 * array in binary, more work to decode than the string it would replace and no
 * more exact. `json`/`jsonb` still have to be parsed as text at the end, and
 * jsonb's binary adds a version byte for nothing. Arrays have a binary form
 * too, but it carries element OIDs and dimension headers, and the text parser
 * already exists and is correct.
 */
const binary: Record<number, Decoder> = {
  [OID.bool]: (bytes, _v, start) => bytes[start] !== 0,
  [OID.int2]: (_b, view, start) => view.getInt16(start),
  [OID.int4]: (_b, view, start) => view.getInt32(start),
  [OID.int8]: (_b, view, start) => {
    const value = view.getBigInt64(start);
    return value >= MIN_SAFE && value <= MAX_SAFE ? Number(value) : value;
  },
  [OID.float4]: (_b, view, start) => view.getFloat32(start),
  [OID.float8]: (_b, view, start) => view.getFloat64(start),
  // Already the bytes. This is the one where text was actively wasteful: hex
  // doubles the size on the wire and then has to be parsed back.
  [OID.bytea]: (bytes, _v, start, length) => bytes.slice(start, start + length),
  [OID.uuid]: (bytes, _v, start) => {
    let hex = "";
    for (let i = 0; i < 16; i++) hex += bytes[start + i]!.toString(16).padStart(2, "0");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  },
  // Microseconds since the PostgreSQL epoch. Divided in BigInt before the
  // conversion to a number, so a timestamp far from now loses microseconds
  // rather than milliseconds.
  [OID.timestamptz]: (_b, view, start) =>
    new Date(Number(view.getBigInt64(start) / 1000n) + PG_EPOCH_MS),
  [OID.timestamp]: (_b, view, start) =>
    new Date(Number(view.getBigInt64(start) / 1000n) + PG_EPOCH_MS),
  // A `date` is a calendar day, not an instant, so it stays the day it was —
  // the same `YYYY-MM-DD` the text format sends. Turning it into a `Date` would
  // force a time zone the value does not have, which is where off-by-one-day
  // bugs come from; and it would make the column change type depending on
  // whether the statement cache happened to be on, which is worse than either
  // choice.
  [OID.date]: (_b, view, start) =>
    new Date(view.getInt32(start) * 86_400_000 + PG_EPOCH_MS).toISOString().slice(0, 10),
};

/** Whether this column is worth asking for in binary. */
export function prefersBinary(oid: number): boolean {
  return binary[oid] !== undefined;
}

/** The decoder for a column of type `oid` in the given wire format. */
export function decoderForFormat(oid: number, format: number): Decoder {
  if (format === 1) {
    const decode = binary[oid];
    if (decode !== undefined) return decode;
  }
  return decoderFor(oid);
}

/** The decoder for a column of type `oid`. */
export function decoderFor(oid: number): Decoder {
  const elementOid = ARRAY_ELEMENT[oid];
  if (elementOid !== undefined) {
    const element = fromText[elementOid] ?? ((t: string) => t);
    return (b, v, s, l) => parseArray(text(b, v, s, l), element);
  }
  // `bool` is one byte and the answer is in it, so it skips the string.
  if (oid === OID.bool) return (bytes, _v, start) => bytes[start] === 0x74;
  const decode = fromText[oid];
  return decode === undefined ? text : (b, v, s, l) => decode(text(b, v, s, l));
}

/** Encodes one parameter as its text representation, or `null` for SQL NULL. */
export function encodeParam(value: unknown): Uint8Array | null {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return ENCODER.encode(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      // NaN and the infinities are float-only in Postgres, and spelled as
      // words. Sending "NaN" for an integer column fails loudly at the server,
      // which is better than binding a zero nobody asked for.
      return ENCODER.encode(Number.isNaN(value) ? "NaN" : value > 0 ? "Infinity" : "-Infinity");
    }
    return ENCODER.encode(String(value));
  }
  if (typeof value === "bigint") return ENCODER.encode(value.toString());
  if (typeof value === "boolean") return ENCODER.encode(value ? "t" : "f");
  if (value instanceof Date) return ENCODER.encode(value.toISOString());
  if (value instanceof Uint8Array) return ENCODER.encode(toHex(value));
  if (ArrayBuffer.isView(value)) {
    return ENCODER.encode(
      toHex(new Uint8Array(value.buffer, value.byteOffset, value.byteLength)),
    );
  }
  if (value instanceof ArrayBuffer) return ENCODER.encode(toHex(new Uint8Array(value)));
  if (Array.isArray(value)) return ENCODER.encode(arrayLiteral(value));
  if (typeof value === "object") return ENCODER.encode(JSON.stringify(value));
  throw new TypeError(`a ${typeof value} cannot be bound as a query parameter`);
}

function toHex(bytes: Uint8Array): string {
  let hex = "\\x";
  for (const byte of bytes) hex += byte.toString(16).padStart(2, "0");
  return hex;
}

/** `[1, 2]` → `{1,2}`, with elements quoted and escaped. */
function arrayLiteral(values: unknown[]): string {
  const parts = values.map((value) => {
    if (value === null || value === undefined) return "NULL";
    if (Array.isArray(value)) return arrayLiteral(value);
    const text = value instanceof Date ? value.toISOString() : String(value);
    return `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  });
  return `{${parts.join(",")}}`;
}
