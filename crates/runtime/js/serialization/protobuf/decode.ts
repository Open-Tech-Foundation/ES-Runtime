// Reflective wire → JS object decoder. Walks a MessageType descriptor. A field
// key appears in the result iff it was present on the wire (sparse objects);
// 64-bit ints are BigInt, enums are value-name strings (unknown numbers kept as
// numbers), bytes are Uint8Array, maps are plain objects, nested are objects.
// Unrecognized fields are preserved under the UNKNOWN symbol for lossless re-encode.
import type { EnumType, Field, FieldType, MessageType, ScalarType } from "./descriptor.js";
import { Reader, WIRE_EGROUP, WIRE_LEN, WIRE_SGROUP } from "./reader.js";

export const UNKNOWN = Symbol.for("esrun.protobuf.unknown");

/** Maximum message-nesting depth honored while decoding, matching the protobuf
 *  default recursion limit. Guards against stack exhaustion from hostile input
 *  (deeply nested sub-messages or groups). */
export const MAX_DEPTH = 100;

function pushUnknown(out: Record<string, unknown>, bytes: Uint8Array): void {
  ((out[UNKNOWN] as Uint8Array[] | undefined) ?? (out[UNKNOWN] = [] as Uint8Array[]) as Uint8Array[]).push(bytes);
}

/** Synthesizes the standalone wire bytes (a varint tag + the original value
 *  varint) for one element of a CLOSED enum whose number is unrecognized, so it
 *  is retained in the unknown-field set rather than surfaced as the field. */
function unknownEnumEntry(fieldNo: number, valueBytes: Uint8Array): Uint8Array {
  const tag: number[] = [];
  let v = fieldNo * 8; // tag = (fieldNo << 3) | 0  (wire type 0); * 8 avoids 32-bit overflow
  while (v > 0x7f) { tag.push((v & 0x7f) | 0x80); v = Math.floor(v / 128); }
  tag.push(v);
  const out = new Uint8Array(tag.length + valueBytes.length);
  out.set(tag, 0);
  out.set(valueBytes, tag.length);
  return out;
}

function expectedWire(type: FieldType): number {
  if (type.kind === "message") return 2;
  if (type.kind === "enum") return 0;
  switch (type.scalar) {
    case "string": case "bytes": return 2;
    case "double": case "fixed64": case "sfixed64": return 1;
    case "float": case "fixed32": case "sfixed32": return 5;
    default: return 0;
  }
}

function readScalar(r: Reader, t: ScalarType): unknown {
  switch (t) {
    case "double": return r.double();
    case "float": return r.float();
    case "int32": return r.int32();
    case "int64": return r.int64();
    case "uint32": return r.uint32();
    case "uint64": return r.uint64();
    case "sint32": return r.sint32();
    case "sint64": return r.sint64();
    case "fixed32": return r.fixed32();
    case "fixed64": return r.fixed64();
    case "sfixed32": return r.sfixed32();
    case "sfixed64": return r.sfixed64();
    case "bool": return r.bool();
    case "string": return r.string();
    case "bytes": return r.bytes();
  }
}

function readEnum(r: Reader, e: EnumType): string | number {
  const n = r.int32();
  return e.byNumber.get(n) ?? n;
}

function readSingle(r: Reader, type: FieldType, depth: number): unknown {
  if (type.kind === "scalar") return readScalar(r, type.scalar);
  if (type.kind === "enum") return readEnum(r, type.enum);
  return decode(type.message, r.fork(), undefined, undefined, depth + 1);
}

/** Clears the other members of `field`'s oneof so only the last one set wins. */
function clearOneof(message: MessageType, out: Record<string, unknown>, field: Field): void {
  for (const num of message.oneofs[field.oneofIndex]!.fieldNumbers) {
    const other = message.fieldByNumber.get(num)!;
    if (other.jsonName !== field.jsonName) delete out[other.jsonName];
  }
}

/** Decodes message fields from `r`. At the top level it reads to EOF; for a
 *  delimited (group) field it reads until the matching end-group tag, which the
 *  caller passes as `groupFieldNo`. */
export function decode(message: MessageType, r: Reader, target?: Record<string, unknown>, groupFieldNo?: number, depth = 0): Record<string, unknown> {
  if (depth > MAX_DEPTH) throw new Error("protobuf: message nesting exceeds maximum depth");
  const out: Record<string, unknown> = target ?? {};
  for (;;) {
    if (r.eof()) {
      if (groupFieldNo !== undefined) throw new Error("protobuf: unexpected end of input inside group");
      break;
    }
    const tagStart = r.pos;
    const tag = r.uint32();
    const fieldNo = tag >>> 3;
    const wire = tag & 7;
    if (wire === WIRE_EGROUP) {
      if (groupFieldNo !== undefined && fieldNo === groupFieldNo) break;
      throw new Error("protobuf: unexpected end-group");
    }
    if (fieldNo === 0) throw new Error("protobuf: invalid field number 0");
    const field: Field | undefined = message.fieldByNumber.get(fieldNo);

    if (!field) {
      r.skip(wire);
      pushUnknown(out, r.buf.slice(tagStart, r.pos));
      continue;
    }

    // Delimited (group-encoded) message field: read inline until the end-group.
    if (field.delimited && field.type.kind === "message" && wire === WIRE_SGROUP) {
      if (field.repeated) {
        const arr = (out[field.jsonName] as unknown[]) ?? (out[field.jsonName] = []);
        arr.push(decode(field.type.message, r, undefined, fieldNo, depth + 1));
      } else {
        if (field.oneofIndex >= 0) clearOneof(message, out, field);
        const existing = out[field.jsonName];
        const into = existing && typeof existing === "object" ? (existing as Record<string, unknown>) : undefined;
        out[field.jsonName] = decode(field.type.message, r, into, fieldNo, depth + 1);
      }
      continue;
    }

    if (field.map) {
      const sub = r.fork();
      let k: unknown = defaultForType(field.map.key.type);
      let v: unknown = defaultForType(field.map.value.type);
      while (!sub.eof()) {
        const t = sub.uint32();
        const n = t >>> 3;
        const w = t & 7;
        if (n === 1) k = readSingle(sub, field.map.key.type, depth);
        else if (n === 2) v = readSingle(sub, field.map.value.type, depth);
        else sub.skip(w);
      }
      const obj = (out[field.jsonName] as Record<string, unknown>) ?? (out[field.jsonName] = {});
      obj[String(k)] = v;
      continue;
    }

    if (field.repeated) {
      const arr = (out[field.jsonName] as unknown[]) ?? (out[field.jsonName] = []);
      const enumType = field.type.kind === "enum" ? field.type.enum : null;
      const packable = enumType !== null || (field.type.kind === "scalar" && field.type.scalar !== "string" && field.type.scalar !== "bytes");
      if (wire === WIRE_LEN && packable) {
        const sub = r.fork();
        while (!sub.eof()) {
          if (enumType) {
            const elemStart = sub.pos;
            const n = sub.int32();
            const name = enumType.byNumber.get(n);
            // CLOSED enums retain an unrecognized number as an (unpacked) unknown field.
            if (name !== undefined) arr.push(name);
            else if (enumType.closed) pushUnknown(out, unknownEnumEntry(field.number, sub.buf.slice(elemStart, sub.pos)));
            else arr.push(n);
          } else {
            arr.push(readScalar(sub, (field.type as { scalar: ScalarType }).scalar));
          }
        }
      } else if (enumType) {
        const n = r.int32();
        const name = enumType.byNumber.get(n);
        if (name !== undefined) arr.push(name);
        else if (enumType.closed) pushUnknown(out, r.buf.slice(tagStart, r.pos));
        else arr.push(n);
      } else {
        arr.push(readSingle(r, field.type, depth));
      }
      continue;
    }

    // singular
    if (wire !== expectedWire(field.type) && !(wire === WIRE_LEN && field.type.kind === "message")) {
      // wire type doesn't match the schema — preserve as unknown rather than misread
      r.skip(wire);
      pushUnknown(out, r.buf.slice(tagStart, r.pos));
      continue;
    }
    // CLOSED enum with an unrecognized number: retain as unknown, leave the field
    // (and any oneof it belongs to) untouched.
    if (field.type.kind === "enum" && field.type.enum.closed) {
      const name = field.type.enum.byNumber.get(r.int32());
      if (name === undefined) { pushUnknown(out, r.buf.slice(tagStart, r.pos)); continue; }
      if (field.oneofIndex >= 0) clearOneof(message, out, field);
      out[field.jsonName] = name;
      continue;
    }
    if (field.oneofIndex >= 0) clearOneof(message, out, field);
    if (field.type.kind === "message") {
      // Repeated occurrences of a singular message field merge (proto spec).
      const existing = out[field.jsonName];
      const into = existing && typeof existing === "object" ? (existing as Record<string, unknown>) : undefined;
      out[field.jsonName] = decode(field.type.message, r.fork(), into, undefined, depth + 1);
    } else {
      out[field.jsonName] = readSingle(r, field.type, depth);
    }
  }
  return out;
}

function defaultForType(type: FieldType): unknown {
  if (type.kind === "scalar") {
    switch (type.scalar) {
      case "int64": case "uint64": case "sint64": case "fixed64": case "sfixed64": return 0n;
      case "bool": return false;
      case "string": return "";
      case "bytes": return new Uint8Array(0);
      default: return 0;
    }
  }
  if (type.kind === "enum") return type.enum.byNumber.get(0) ?? 0;
  return {}; // message: default to an empty message (e.g. map<…, Message> missing value)
}
