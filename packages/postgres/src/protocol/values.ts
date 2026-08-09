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

const decoders: Record<number, Decoder> = {
  [OID.bool]: (bytes, _v, start) => bytes[start] === 0x74, // 't'
  [OID.int2]: (b, v, s, l) => Number(text(b, v, s, l)),
  [OID.int4]: (b, v, s, l) => Number(text(b, v, s, l)),
  [OID.int8]: (b, v, s, l) => {
    // A bigint only where a number would lose the value: `row.id + 1` should
    // work for the ids people actually have, and stay exact for the ones they
    // do not.
    const value = BigInt(text(b, v, s, l));
    return value >= MIN_SAFE && value <= MAX_SAFE ? Number(value) : value;
  },
  [OID.float4]: (b, v, s, l) => Number(text(b, v, s, l)),
  [OID.float8]: (b, v, s, l) => Number(text(b, v, s, l)),
  // `numeric` stays a string. It is arbitrary precision by definition, and a
  // double is the one representation guaranteed to lose it — a column chosen
  // for exactness should not be rounded on the way out.
  [OID.numeric]: text,
  [OID.json]: (b, v, s, l) => JSON.parse(text(b, v, s, l)),
  [OID.jsonb]: (b, v, s, l) => JSON.parse(text(b, v, s, l)),
  [OID.timestamptz]: (b, v, s, l) => new Date(text(b, v, s, l)),
  [OID.timestamp]: (b, v, s, l) => new Date(`${text(b, v, s, l)}Z`),
  [OID.bytea]: (b, v, s, l) => {
    const hex = text(b, v, s, l);
    // `\x` hex is the modern output format; the legacy escape format is not
    // produced by any supported server version.
    if (!hex.startsWith("\\x")) return ENCODER.encode(hex);
    const out = new Uint8Array((hex.length - 2) / 2);
    for (let i = 0; i < out.length; i++) {
      out[i] = Number.parseInt(hex.substr(2 + i * 2, 2), 16);
    }
    return out;
  },
};

/** The decoder for a column of type `oid`. */
export function decoderFor(oid: number): Decoder {
  return decoders[oid] ?? text;
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
