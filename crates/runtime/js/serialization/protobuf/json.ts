// Proto3 JSON mapping, reflective. Converts between the in-memory value shape
// produced/consumed by decode.ts / encode.ts (camelCase keys, BigInt 64-bit,
// enum value-name strings, Uint8Array bytes, maps as plain objects) and the
// canonical proto3-JSON representation (64-bit as strings, bytes as base64,
// enums as names, and the well-known-type special forms — Timestamp/Duration as
// strings, wrappers as bare values, Struct/Value/ListValue as native JSON, Any
// with an "@type" member, FieldMask as a comma path string, Empty as {}).
//
// JSON is just a third representation pivoting on the same value shape, so the
// binary codec is reused unchanged: fromJson → value → encode, decode → value →
// toJson. Parsing is strict per the proto3 JSON spec (typed/range-checked
// scalars, well-formed UTF-16 strings, duplicate-oneof and unknown-field
// rejection); serialization validates the WKT range/round-trip rules.
import {
  BIGINT_SCALARS, scalarDefault,
  type EnumType, type Field, type FieldType, type MessageType, type ScalarType,
} from "./descriptor.js";
import type { Registry } from "./link.js";
import { decode } from "./decode.js";
import { encode } from "./encode.js";
import { Reader } from "./reader.js";
import { Writer } from "./writer.js";

export type JsonValue =
  | null | boolean | number | string | JsonValue[] | { [k: string]: JsonValue };

export interface FromJsonOptions {
  /** Ignore unrecognized fields and unknown enum-name strings instead of
   *  throwing (proto3's lenient parsing mode). */
  ignoreUnknownFields?: boolean;
}

interface Ctx {
  registry: Registry;
  ignoreUnknown: boolean;
}

/** Returned by the parsers to mean "drop this value" (an ignored unknown enum
 *  string under `ignoreUnknownFields`). */
const DROP = Symbol("drop");

const WRAPPERS: ReadonlySet<string> = new Set([
  "google.protobuf.DoubleValue", "google.protobuf.FloatValue",
  "google.protobuf.Int64Value", "google.protobuf.UInt64Value",
  "google.protobuf.Int32Value", "google.protobuf.UInt32Value",
  "google.protobuf.BoolValue", "google.protobuf.StringValue",
  "google.protobuf.BytesValue",
]);

// WKTs whose JSON form is not a plain proto message object — inside an Any their
// payload sits under a "value" member rather than being spread alongside "@type".
const SPECIAL_JSON_WKT: ReadonlySet<string> = new Set([
  "google.protobuf.Timestamp", "google.protobuf.Duration",
  "google.protobuf.FieldMask", "google.protobuf.Value",
  "google.protobuf.ListValue", "google.protobuf.Struct",
  "google.protobuf.Any", ...WRAPPERS,
]);

// Scalar value ranges.
const I32_MIN = -2147483648, I32_MAX = 2147483647, U32_MAX = 4294967295;
const I64_MIN = -9223372036854775808n, I64_MAX = 9223372036854775807n;
const U64_MAX = 18446744073709551615n;
// Timestamp seconds [0001-01-01T00:00:00Z, 9999-12-31T23:59:59Z]; Duration ±10000y.
const TS_MIN = -62135596800n, TS_MAX = 253402300799n, DUR_MAX = 315576000000n;
// A JSON number literal (no leading zeros / spaces), optionally fractional/exponent.
const NUM_RE = /^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$/;

// ── base64 (standard alphabet on output; accepts url-safe / unpadded in) ──────
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64INV = /* @__PURE__ */ (() => {
  const t = new Int16Array(256).fill(-1);
  for (let i = 0; i < B64.length; i++) t[B64.charCodeAt(i)] = i;
  return t;
})();

function base64Encode(bytes: Uint8Array): string {
  let out = "";
  let i = 0;
  for (; i + 2 < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8) | bytes[i + 2]!;
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + B64[(n >> 6) & 63]! + B64[n & 63]!;
  }
  const rem = bytes.length - i;
  if (rem === 1) {
    const n = bytes[i]! << 16;
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + "==";
  } else if (rem === 2) {
    const n = (bytes[i]! << 16) | (bytes[i + 1]! << 8);
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]! + B64[(n >> 6) & 63]! + "=";
  }
  return out;
}

function base64Decode(s: string): Uint8Array {
  const out: number[] = [];
  let bits = 0;
  let nbits = 0;
  for (let i = 0; i < s.length; i++) {
    const ch = s.charCodeAt(i);
    if (ch === 0x3d) break; // '='
    if (ch === 0x20 || ch === 0x0a || ch === 0x0d || ch === 0x09) continue;
    const v = ch === 0x2d ? 62 : ch === 0x5f ? 63 : B64INV[ch]!; // '-'→+ , '_'→/
    if (v < 0) throw new Error("protobuf: invalid base64 in JSON");
    bits = (bits << 6) | v;
    nbits += 6;
    if (nbits >= 8) { nbits -= 8; out.push((bits >> nbits) & 0xff); }
  }
  return new Uint8Array(out);
}

// ── field-name case conversion (FieldMask paths) ──────────────────────────────
function camelCase(s: string): string {
  let out = "";
  let up = false;
  for (const c of s) {
    if (c === "_") up = true;
    else { out += up ? c.toUpperCase() : c; up = false; }
  }
  return out;
}

function snakeCase(s: string): string {
  return s.replace(/[A-Z]/g, (c) => "_" + c.toLowerCase());
}

// ── strict scalar parsing ─────────────────────────────────────────────────────
function parseIntStrict(j: JsonValue, min: number, max: number): number {
  let n: number;
  if (typeof j === "number") n = j;
  else if (typeof j === "string") {
    if (!NUM_RE.test(j)) throw new Error(`protobuf: invalid integer ${JSON.stringify(j)}`);
    n = Number(j);
  } else throw new Error("protobuf: integer field expects a number or string");
  if (!Number.isInteger(n)) throw new Error(`protobuf: ${n} is not an integer`);
  if (n < min || n > max) throw new Error(`protobuf: integer ${n} out of range`);
  return n;
}

function parseBigIntStrict(j: JsonValue, min: bigint, max: bigint): bigint {
  let b: bigint;
  if (typeof j === "number") {
    if (!Number.isInteger(j)) throw new Error(`protobuf: ${j} is not an integer`);
    b = BigInt(j);
  } else if (typeof j === "string") {
    if (/^-?(?:0|[1-9][0-9]*)$/.test(j)) b = BigInt(j);
    else {
      if (!NUM_RE.test(j)) throw new Error(`protobuf: invalid integer ${JSON.stringify(j)}`);
      const n = Number(j);
      if (!Number.isInteger(n)) throw new Error(`protobuf: ${j} is not an integer`);
      b = BigInt(n);
    }
  } else throw new Error("protobuf: integer field expects a number or string");
  if (b < min || b > max) throw new Error(`protobuf: integer ${b} out of range`);
  return b;
}

const FLOAT_MAX = 3.4028234663852886e38;

function parseFloatStrict(j: JsonValue, isFloat: boolean): number {
  let n: number;
  if (typeof j === "number") n = j;
  else if (typeof j === "string") {
    if (j === "NaN") return NaN;
    if (j === "Infinity") return Infinity;
    if (j === "-Infinity") return -Infinity;
    if (!NUM_RE.test(j)) throw new Error(`protobuf: invalid number ${JSON.stringify(j)}`);
    n = Number(j);
  } else throw new Error("protobuf: float field expects a number or string");
  if (!Number.isFinite(n)) throw new Error("protobuf: number out of range");
  if (isFloat && Math.abs(n) > FLOAT_MAX) throw new Error("protobuf: float out of range");
  return n;
}

// Reject lone / mis-ordered UTF-16 surrogates (an ill-formed string).
function validateUtf16(s: string): void {
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff) {
      const next = s.charCodeAt(i + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) throw new Error("protobuf: unpaired UTF-16 surrogate in string");
      i++;
    } else if (c >= 0xdc00 && c <= 0xdfff) {
      throw new Error("protobuf: unpaired UTF-16 surrogate in string");
    }
  }
}

// ── Timestamp / Duration string forms ─────────────────────────────────────────
function nanosFraction(nanos: number): string {
  const s = String(nanos).padStart(9, "0");
  if (s.endsWith("000000")) return s.slice(0, 3);
  if (s.endsWith("000")) return s.slice(0, 6);
  return s;
}

function timestampToJson(value: Record<string, unknown>): string {
  const seconds = BigInt((value.seconds as bigint | number | undefined) ?? 0);
  const nanos = Number(value.nanos ?? 0);
  if (seconds < TS_MIN || seconds > TS_MAX) throw new Error("protobuf: Timestamp seconds out of range");
  if (nanos < 0 || nanos > 999999999) throw new Error("protobuf: Timestamp nanos out of range");
  const date = new Date(Number(seconds) * 1000);
  const base = date.toISOString().slice(0, 19); // YYYY-MM-DDTHH:MM:SS
  return base + (nanos ? "." + nanosFraction(nanos) : "") + "Z";
}

function timestampFromJson(j: JsonValue): Record<string, unknown> {
  if (typeof j !== "string") throw new Error("protobuf: Timestamp JSON must be a string");
  const m = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d+))?(Z|[+-]\d{2}:\d{2})$/.exec(j);
  if (!m) throw new Error(`protobuf: invalid Timestamp "${j}"`);
  const ms = Date.parse(m[1]! + (m[3] === "Z" ? "Z" : m[3]!));
  if (Number.isNaN(ms)) throw new Error(`protobuf: invalid Timestamp "${j}"`);
  const seconds = BigInt(Math.floor(ms / 1000));
  const nanos = m[2] ? Number(m[2].padEnd(9, "0").slice(0, 9)) : 0;
  if (seconds < TS_MIN || seconds > TS_MAX) throw new Error(`protobuf: Timestamp "${j}" out of range`);
  const out: Record<string, unknown> = {};
  if (seconds !== 0n) out.seconds = seconds;
  if (nanos !== 0) out.nanos = nanos;
  return out;
}

function durationToJson(value: Record<string, unknown>): string {
  const seconds = BigInt((value.seconds as bigint | number | undefined) ?? 0);
  const nanos = Number(value.nanos ?? 0);
  if (seconds < -DUR_MAX || seconds > DUR_MAX) throw new Error("protobuf: Duration seconds out of range");
  if (nanos < -999999999 || nanos > 999999999) throw new Error("protobuf: Duration nanos out of range");
  if (seconds !== 0n && nanos !== 0 && seconds < 0n !== nanos < 0) {
    throw new Error("protobuf: Duration seconds and nanos must share sign");
  }
  const neg = seconds < 0n || nanos < 0;
  const absSec = seconds < 0n ? -seconds : seconds;
  const absNanos = Math.abs(nanos);
  return (neg ? "-" : "") + absSec.toString() + (absNanos ? "." + nanosFraction(absNanos) : "") + "s";
}

function durationFromJson(j: JsonValue): Record<string, unknown> {
  if (typeof j !== "string") throw new Error("protobuf: Duration JSON must be a string");
  const m = /^(-)?(\d+)(?:\.(\d+))?s$/.exec(j);
  if (!m) throw new Error(`protobuf: invalid Duration "${j}"`);
  let seconds = BigInt(m[2]!);
  let nanos = m[3] ? Number(m[3].padEnd(9, "0").slice(0, 9)) : 0;
  if (m[1] === "-") { seconds = -seconds; nanos = -nanos; }
  if (seconds < -DUR_MAX || seconds > DUR_MAX) throw new Error(`protobuf: Duration "${j}" out of range`);
  const out: Record<string, unknown> = {};
  if (seconds !== 0n) out.seconds = seconds;
  if (nanos !== 0) out.nanos = nanos;
  return out;
}

// ── Struct / Value / ListValue ↔ native JSON ─────────────────────────────────
function valueToJson(value: Record<string, unknown>, registry: Registry): JsonValue {
  if ("nullValue" in value) return null;
  if ("numberValue" in value) {
    const n = value.numberValue as number;
    if (!Number.isFinite(n)) throw new Error("protobuf: Value number_value must be finite");
    return n;
  }
  if ("stringValue" in value) return value.stringValue as string;
  if ("boolValue" in value) return value.boolValue as boolean;
  if ("structValue" in value) return structToJson(value.structValue as Record<string, unknown>, registry);
  if ("listValue" in value) return listValueToJson(value.listValue as Record<string, unknown>, registry);
  return null;
}

function valueFromJson(j: JsonValue, ctx: Ctx): Record<string, unknown> {
  if (j === null) return { nullValue: "NULL_VALUE" };
  switch (typeof j) {
    case "number":
      if (!Number.isFinite(j)) throw new Error("protobuf: Value number must be finite");
      return { numberValue: j };
    case "string": return { stringValue: j };
    case "boolean": return { boolValue: j };
  }
  if (Array.isArray(j)) return { listValue: { values: j.map((e) => valueFromJson(e, ctx)) } };
  return { structValue: structFromJson(j as { [k: string]: JsonValue }, ctx) };
}

function structToJson(value: Record<string, unknown>, registry: Registry): JsonValue {
  const out: Record<string, JsonValue> = {};
  const fields = value.fields as Record<string, Record<string, unknown>> | undefined;
  if (fields) for (const [k, v] of Object.entries(fields)) out[k] = valueToJson(v, registry);
  return out;
}

function structFromJson(j: { [k: string]: JsonValue }, ctx: Ctx): Record<string, unknown> {
  const fields: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(j)) fields[k] = valueFromJson(v, ctx);
  return { fields };
}

function listValueToJson(value: Record<string, unknown>, registry: Registry): JsonValue {
  const vals = (value.values as Record<string, unknown>[] | undefined) ?? [];
  return vals.map((v) => valueToJson(v, registry));
}

function listValueFromJson(j: JsonValue, ctx: Ctx): Record<string, unknown> {
  if (!Array.isArray(j)) throw new Error("protobuf: ListValue JSON must be an array");
  return { values: j.map((e) => valueFromJson(e, ctx)) };
}

// ── FieldMask ↔ comma-joined camelCase path string ───────────────────────────
function fieldMaskToJson(value: Record<string, unknown>): string {
  const paths = (value.paths as string[] | undefined) ?? [];
  return paths.map((p) => {
    const camel = p.split(".").map(camelCase).join(".");
    // A path must be lower-snake-case so it survives the camel↔snake round trip.
    if (camel.split(".").map(snakeCase).join(".") !== p) {
      throw new Error(`protobuf: FieldMask path "${p}" is not a valid field path`);
    }
    return camel;
  }).join(",");
}

function fieldMaskFromJson(j: JsonValue): Record<string, unknown> {
  if (typeof j !== "string") throw new Error("protobuf: FieldMask JSON must be a string");
  if (j === "") return {};
  return {
    paths: j.split(",").map((p) => {
      const snake = p.split(".").map(snakeCase).join(".");
      // The JSON path must be lower-camelCase (no underscores / stray case).
      if (snake.split(".").map(camelCase).join(".") !== p) {
        throw new Error(`protobuf: invalid FieldMask path "${p}"`);
      }
      return snake;
    }),
  };
}

// ── wrappers ↔ bare value ────────────────────────────────────────────────────
function wrapperToJson(message: MessageType, value: Record<string, unknown>): JsonValue {
  const field = message.fields[0]!;
  const scalar = (field.type as { scalar: ScalarType }).scalar;
  const v = value[field.jsonName];
  return scalarToJson(scalar, v === undefined ? scalarDefault(scalar) : v);
}

function wrapperFromJson(message: MessageType, j: JsonValue): Record<string, unknown> {
  const field = message.fields[0]!;
  const scalar = (field.type as { scalar: ScalarType }).scalar;
  return { [field.jsonName]: scalarFromJson(scalar, j) };
}

// ── Any ↔ { "@type": …, … } ───────────────────────────────────────────────────
function typeName(typeUrl: string): string {
  return typeUrl.slice(typeUrl.lastIndexOf("/") + 1);
}

function anyToJson(value: Record<string, unknown>, registry: Registry): JsonValue {
  const typeUrl = value.typeUrl as string | undefined;
  if (!typeUrl) return {};
  const name = typeName(typeUrl);
  const m = registry.messages.get(name);
  if (!m) throw new Error(`protobuf: Any references unknown type "${typeUrl}"`);
  const bytes = (value.value as Uint8Array | undefined) ?? new Uint8Array(0);
  const inner = messageToJson(m, decode(m, new Reader(bytes)), registry);
  // WKTs with a non-message JSON form nest under "value"; plain messages spread.
  if (SPECIAL_JSON_WKT.has(name)) return { "@type": typeUrl, value: inner };
  return { "@type": typeUrl, ...(inner as { [k: string]: JsonValue }) };
}

function anyFromJson(j: JsonValue, ctx: Ctx): Record<string, unknown> {
  if (j === null || typeof j !== "object" || Array.isArray(j)) {
    throw new Error("protobuf: Any JSON must be an object");
  }
  const typeUrl = j["@type"] as string | undefined;
  if (!typeUrl) return {};
  const name = typeName(typeUrl);
  const m = ctx.registry.messages.get(name);
  if (!m) throw new Error(`protobuf: Any references unknown type "${typeUrl}"`);
  let innerValue: Record<string, unknown>;
  if (SPECIAL_JSON_WKT.has(name)) {
    innerValue = messageFromJsonCtx(m, j.value ?? null, ctx);
  } else {
    const { "@type": _omit, ...rest } = j;
    innerValue = messageFromJsonCtx(m, rest, ctx);
  }
  const w = new Writer();
  encode(m, innerValue, w);
  return { typeUrl, value: w.finish() };
}

// ── scalars / enums ───────────────────────────────────────────────────────────
function scalarToJson(t: ScalarType, v: unknown): JsonValue {
  if (BIGINT_SCALARS.has(t)) return String(v as bigint);
  switch (t) {
    case "double": case "float": {
      const n = v as number;
      if (Number.isFinite(n)) return n;
      if (Number.isNaN(n)) return "NaN";
      return n > 0 ? "Infinity" : "-Infinity";
    }
    case "bytes": return base64Encode(v as Uint8Array);
    default: return v as JsonValue; // int32/uint32/sint32/fixed32/sfixed32/bool/string
  }
}

function scalarFromJson(t: ScalarType, j: JsonValue): unknown {
  switch (t) {
    case "int32": case "sint32": case "sfixed32": return parseIntStrict(j, I32_MIN, I32_MAX);
    case "uint32": case "fixed32": return parseIntStrict(j, 0, U32_MAX);
    case "int64": case "sint64": case "sfixed64": return parseBigIntStrict(j, I64_MIN, I64_MAX);
    case "uint64": case "fixed64": return parseBigIntStrict(j, 0n, U64_MAX);
    case "double": return parseFloatStrict(j, false);
    case "float": return parseFloatStrict(j, true);
    case "bool":
      if (typeof j !== "boolean") throw new Error("protobuf: bool field expects true or false");
      return j;
    case "string":
      if (typeof j !== "string") throw new Error("protobuf: string field expects a string");
      validateUtf16(j);
      return j;
    case "bytes":
      if (typeof j !== "string") throw new Error("protobuf: bytes field expects a base64 string");
      return base64Decode(j);
  }
}

function enumToJson(e: EnumType, v: unknown): JsonValue {
  if (e.fullName === "google.protobuf.NullValue") return null;
  return v as string | number; // name if known, number if unrecognized
}

function enumFromJson(e: EnumType, j: JsonValue, ctx: Ctx): unknown {
  if (e.fullName === "google.protobuf.NullValue") {
    if (j !== null && j !== "NULL_VALUE") throw new Error("protobuf: NullValue expects null");
    return "NULL_VALUE";
  }
  if (typeof j === "number") {
    // Match decode's value shape: known numbers surface as their name string.
    return e.byNumber.get(j) ?? j;
  }
  if (typeof j === "string") {
    if (e.byName.has(j)) return j;
    if (ctx.ignoreUnknown) return DROP;
    throw new Error(`protobuf: unknown enum value "${j}" for ${e.fullName}`);
  }
  throw new Error(`protobuf: enum field expects a name string or number`);
}

// ── generic message walk: value → JSON ────────────────────────────────────────
function singleToJson(type: FieldType, v: unknown, registry: Registry): JsonValue {
  if (type.kind === "scalar") return scalarToJson(type.scalar, v);
  if (type.kind === "enum") return enumToJson(type.enum, v);
  return messageToJson(type.message, v as Record<string, unknown>, registry);
}

function fieldToJson(field: Field, v: unknown, registry: Registry): JsonValue {
  if (field.map) {
    const out: Record<string, JsonValue> = {};
    for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
      out[k] = singleToJson(field.map.value.type, val, registry);
    }
    return out;
  }
  if (field.repeated) return (v as unknown[]).map((e) => singleToJson(field.type, e, registry));
  return singleToJson(field.type, v, registry);
}

// Whether an implicit-presence field at its default value is omitted on output.
function isJsonOmittable(field: Field, v: unknown): boolean {
  if (field.explicitPresence) return false;
  if (field.map) return typeof v === "object" && v !== null && Object.keys(v).length === 0;
  if (field.repeated) return Array.isArray(v) && v.length === 0;
  if (field.type.kind === "scalar") {
    const t = field.type.scalar;
    if (BIGINT_SCALARS.has(t)) return BigInt(v as bigint | number) === 0n;
    switch (t) {
      case "bool": return v === false;
      case "string": return v === "";
      case "bytes": return (v as Uint8Array).length === 0;
      default: return v === 0;
    }
  }
  if (field.type.kind === "enum") {
    const e = field.type.enum;
    return typeof v === "number" ? v === 0 : v === e.byNumber.get(0);
  }
  return false; // singular message: presence is meaningful
}

export function messageToJson(message: MessageType, value: Record<string, unknown>, registry: Registry): JsonValue {
  switch (message.fullName) {
    case "google.protobuf.Timestamp": return timestampToJson(value);
    case "google.protobuf.Duration": return durationToJson(value);
    case "google.protobuf.FieldMask": return fieldMaskToJson(value);
    case "google.protobuf.Struct": return structToJson(value, registry);
    case "google.protobuf.Value": return valueToJson(value, registry);
    case "google.protobuf.ListValue": return listValueToJson(value, registry);
    case "google.protobuf.Any": return anyToJson(value, registry);
    case "google.protobuf.Empty": return {};
  }
  if (WRAPPERS.has(message.fullName)) return wrapperToJson(message, value);

  const out: Record<string, JsonValue> = {};
  for (const field of message.fields) {
    const v = value[field.jsonName];
    if (v === undefined) continue;
    if (isJsonOmittable(field, v)) continue;
    out[field.jsonName] = fieldToJson(field, v, registry);
  }
  return out;
}

// ── generic message walk: JSON → value ────────────────────────────────────────
function singleFromJson(type: FieldType, j: JsonValue, ctx: Ctx): unknown {
  if (j === null) {
    // Only Value and NullValue accept JSON null in element/value position.
    if (type.kind === "message" && type.message.fullName === "google.protobuf.Value") return valueFromJson(null, ctx);
    if (type.kind === "enum" && type.enum.fullName === "google.protobuf.NullValue") return "NULL_VALUE";
    throw new Error("protobuf: null is not a valid value here");
  }
  if (type.kind === "scalar") return scalarFromJson(type.scalar, j);
  if (type.kind === "enum") return enumFromJson(type.enum, j, ctx);
  return messageFromJsonCtx(type.message, j, ctx);
}

function fieldFromJson(field: Field, j: JsonValue, ctx: Ctx): unknown {
  if (field.map) {
    if (typeof j !== "object" || j === null || Array.isArray(j)) {
      throw new Error(`protobuf: map field "${field.name}" JSON must be an object`);
    }
    const out: Record<string, unknown> = {};
    for (const [k, val] of Object.entries(j)) {
      const v = singleFromJson(field.map.value.type, val, ctx);
      if (v !== DROP) out[k] = v;
    }
    return out;
  }
  if (field.repeated) {
    if (!Array.isArray(j)) throw new Error(`protobuf: repeated field "${field.name}" JSON must be an array`);
    const out: unknown[] = [];
    for (const e of j) {
      const v = singleFromJson(field.type, e, ctx);
      if (v !== DROP) out.push(v);
    }
    return out;
  }
  return singleFromJson(field.type, j, ctx);
}

function isNullableField(type: FieldType): boolean {
  return (type.kind === "message" && type.message.fullName === "google.protobuf.Value")
    || (type.kind === "enum" && type.enum.fullName === "google.protobuf.NullValue");
}

function messageFromJsonCtx(message: MessageType, j: JsonValue, ctx: Ctx): Record<string, unknown> {
  switch (message.fullName) {
    case "google.protobuf.Timestamp": return timestampFromJson(j);
    case "google.protobuf.Duration": return durationFromJson(j);
    case "google.protobuf.FieldMask": return fieldMaskFromJson(j);
    case "google.protobuf.Struct":
      if (j === null || typeof j !== "object" || Array.isArray(j)) throw new Error("protobuf: Struct JSON must be an object");
      return structFromJson(j, ctx);
    case "google.protobuf.Value": return valueFromJson(j, ctx);
    case "google.protobuf.ListValue": return listValueFromJson(j, ctx);
    case "google.protobuf.Any": return anyFromJson(j, ctx);
    case "google.protobuf.Empty": return {};
  }
  if (WRAPPERS.has(message.fullName)) return wrapperFromJson(message, j);

  if (j === null || typeof j !== "object" || Array.isArray(j)) {
    throw new Error(`protobuf: message "${message.fullName}" JSON must be an object`);
  }

  const out: Record<string, unknown> = {};
  const seenOneof = new Set<number>();
  for (const [k, val] of Object.entries(j)) {
    const field = message.fields.find((f) => f.jsonName === k || f.name === k);
    if (!field) {
      if (ctx.ignoreUnknown) continue;
      throw new Error(`protobuf: unknown field "${k}" in ${message.fullName}`);
    }
    // A JSON null clears any field to its default — leave it absent and let it
    // claim nothing (so a nulled oneof field doesn't conflict). Only Value and
    // NullValue treat null as a meaningful value.
    if (val === null && !isNullableField(field.type)) continue;
    if (field.oneofIndex >= 0) {
      if (seenOneof.has(field.oneofIndex)) throw new Error(`protobuf: multiple values for oneof "${message.oneofs[field.oneofIndex]!.name}"`);
      seenOneof.add(field.oneofIndex);
    }
    const v = fieldFromJson(field, val, ctx);
    if (v !== DROP) out[field.jsonName] = v;
  }
  return out;
}

export function messageFromJson(
  message: MessageType,
  j: JsonValue,
  registry: Registry,
  opts: FromJsonOptions = {},
): Record<string, unknown> {
  return messageFromJsonCtx(message, j, { registry, ignoreUnknown: opts.ignoreUnknownFields ?? false });
}
