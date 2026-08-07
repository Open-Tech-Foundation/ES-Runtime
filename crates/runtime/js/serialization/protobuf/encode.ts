// Reflective JS object → wire encoder. Inverse of decode.ts: honors resolved
// presence (implicit-presence scalars at default are omitted) and packed
// encoding, re-emits preserved unknown fields.
import type { EnumType, Field, FieldType, MessageType, ScalarType } from "./descriptor.js";
import { WIRE_EGROUP, WIRE_LEN, WIRE_SGROUP } from "./reader.js";
import { Writer } from "./writer.js";
import { UNKNOWN } from "./decode.js";

function writeScalarRaw(w: Writer, t: ScalarType, v: unknown): void {
  switch (t) {
    case "double": w.double(v as number); break;
    case "float": w.float(v as number); break;
    case "int32": w.int32(v as number); break;
    case "int64": w.varint64(BigInt(v as bigint | number)); break;
    case "uint32": w.uint32(v as number); break;
    case "uint64": w.varint64(BigInt(v as bigint | number)); break;
    case "sint32": w.sint32(v as number); break;
    case "sint64": w.sint64(BigInt(v as bigint | number)); break;
    case "fixed32": w.fixed32(v as number); break;
    case "fixed64": w.fixed64(BigInt(v as bigint | number)); break;
    case "sfixed32": w.sfixed32(v as number); break;
    case "sfixed64": w.sfixed64(BigInt(v as bigint | number)); break;
    case "bool": w.bool(v as boolean); break;
    case "string": w.string(v as string); break;
    case "bytes": w.bytes(v as Uint8Array); break;
  }
}

function enumNumber(e: EnumType, v: unknown): number {
  if (typeof v === "number") return v;
  const n = e.byName.get(v as string);
  if (n === undefined) throw new Error(`protobuf: unknown enum value "${String(v)}" for ${e.fullName}`);
  return n;
}

function wireFor(type: FieldType): number {
  if (type.kind === "message") return 2;
  if (type.kind === "enum") return 0;
  switch (type.scalar) {
    case "string": case "bytes": return 2;
    case "double": case "fixed64": case "sfixed64": return 1;
    case "float": case "fixed32": case "sfixed32": return 5;
    default: return 0;
  }
}

/** Writes one value (tag + payload) for a non-repeated field. */
function writeField(w: Writer, field: Field, v: unknown, opts?: EncodeOptions): void {
  const type = field.type;
  if (type.kind === "message") {
    if (field.delimited) {
      // Group encoding: start-group tag, fields inline, matching end-group tag.
      w.tag(field.number, WIRE_SGROUP);
      encode(type.message, v as Record<string, unknown>, w, opts);
      w.tag(field.number, WIRE_EGROUP);
      return;
    }
    const child = new Writer();
    encode(type.message, v as Record<string, unknown>, child, opts);
    w.lenDelimited(field.number, child);
    return;
  }
  if (type.kind === "enum") {
    w.tag(field.number, 0);
    w.int32(enumNumber(type.enum, v));
    return;
  }
  w.tag(field.number, wireFor(type));
  writeScalarRaw(w, type.scalar, v);
}

function isDefault(type: FieldType, v: unknown): boolean {
  if (type.kind === "scalar") {
    switch (type.scalar) {
      case "int64": case "uint64": case "sint64": case "fixed64": case "sfixed64":
        return BigInt(v as bigint | number) === 0n;
      case "bool": return v === false;
      case "string": return v === "";
      case "bytes": return (v as Uint8Array).length === 0;
      default: return v === 0;
    }
  }
  if (type.kind === "enum") {
    const n = enumNumber(type.enum, v);
    return n === 0;
  }
  return false;
}

function mapKeyTyped(t: ScalarType, k: string): unknown {
  switch (t) {
    case "string": return k;
    case "bool": return k === "true";
    case "int64": case "uint64": case "sint64": case "fixed64": case "sfixed64": return BigInt(k);
    default: return Number(k);
  }
}

export interface EncodeOptions {
  /** Accept and ignore object keys that match no field, instead of throwing. */
  ignoreUnknownFields?: boolean;
}

/**
 * Reads the value for `field`, accepting either spelling of its name.
 *
 * The proto3-JSON mapping requires parsers to accept both the original proto
 * field name and the lowerCamelCase `jsonName` — which is also what this
 * package's own `fromJson` and `decodeStream` already do. Reading `jsonName`
 * alone meant a message written with the field names as they appear in the
 * `.proto` (`user_name`) matched nothing and encoded to an empty buffer.
 *
 * Supplying both spellings at once is rejected rather than resolved, as the JSON
 * mapping requires: the two could carry different values, and silently picking
 * one is how the original bug behaved.
 */
function read(field: Field, value: Record<string, unknown>, seen: Set<string>): unknown {
  const byJson = Object.hasOwn(value, field.jsonName);
  const byProto = field.name !== field.jsonName && Object.hasOwn(value, field.name);
  if (byJson && byProto) {
    throw new Error(
      `protobuf: field "${field.name}" given twice, as "${field.jsonName}" and "${field.name}"`,
    );
  }
  if (byJson) seen.add(field.jsonName);
  if (byProto) seen.add(field.name);
  return byProto ? value[field.name] : value[field.jsonName];
}

export function encode(
  message: MessageType,
  value: Record<string, unknown>,
  w: Writer,
  opts?: EncodeOptions,
): void {
  // Which input keys a field claimed, so the leftovers can be reported. A key
  // that matches nothing used to be dropped in silence, so a single typo
  // produced a short or empty buffer with no indication anything was lost.
  const seen = new Set<string>();

  for (const field of message.fields) {
    const v = read(field, value, seen);

    if (field.map) {
      if (v == null) continue;
      const keyType = field.map.key.type as { kind: "scalar"; scalar: ScalarType };
      for (const [k, val] of Object.entries(v as Record<string, unknown>)) {
        const entry = new Writer();
        writeField(entry, field.map.key, mapKeyTyped(keyType.scalar, k));
        writeField(entry, field.map.value, val, opts);
        w.lenDelimited(field.number, entry);
      }
      continue;
    }

    if (field.repeated) {
      if (!Array.isArray(v) || v.length === 0) continue;
      if (field.packed) {
        const child = new Writer();
        if (field.type.kind === "enum") {
          for (const e of v) child.int32(enumNumber(field.type.enum, e));
        } else {
          const scalar = (field.type as { scalar: ScalarType }).scalar;
          for (const e of v) writeScalarRaw(child, scalar, e);
        }
        w.tag(field.number, WIRE_LEN);
        w.uint32(child.length);
        w.raw(child.finish());
      } else {
        for (const e of v) writeField(w, field, e, opts);
      }
      continue;
    }

    // singular
    if (v === undefined || v === null) continue;
    if (!field.explicitPresence && isDefault(field.type, v)) continue;
    writeField(w, field, v, opts);
  }

  if (!opts?.ignoreUnknownFields) {
    // `UNKNOWN` is a symbol, so the preserved-unknown-fields key a decoded
    // message carries is not an own *string* key and never lands here — a
    // decode/encode round-trip stays clean.
    for (const k of Object.keys(value)) {
      if (!seen.has(k)) {
        throw new Error(`protobuf: unknown field "${k}" in ${message.fullName}`);
      }
    }
  }

  // re-emit preserved unknown fields
  const unknown = value[UNKNOWN] as Uint8Array[] | undefined;
  if (unknown) for (const raw of unknown) w.raw(raw);
}
