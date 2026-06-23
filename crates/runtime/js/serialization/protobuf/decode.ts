// Reflective wire → JS object decoder. Walks a MessageType descriptor. A field
// key appears in the result iff it was present on the wire (sparse objects);
// 64-bit ints are BigInt, enums are value-name strings (unknown numbers kept as
// numbers), bytes are Uint8Array, maps are plain objects, nested are objects.
// Unrecognized fields are preserved under the UNKNOWN symbol for lossless re-encode.
import type { EnumType, Field, FieldType, MessageType, ScalarType } from "./descriptor.js";
import { Reader, WIRE_LEN } from "./reader.js";

export const UNKNOWN = Symbol.for("esrun.protobuf.unknown");

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

function readSingle(r: Reader, type: FieldType): unknown {
  if (type.kind === "scalar") return readScalar(r, type.scalar);
  if (type.kind === "enum") return readEnum(r, type.enum);
  return decode(type.message, r.fork());
}

export function decode(message: MessageType, r: Reader, target?: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = target ?? {};
  while (!r.eof()) {
    const tagStart = r.pos;
    const tag = r.uint32();
    const fieldNo = tag >>> 3;
    const wire = tag & 7;
    if (fieldNo === 0) throw new Error("protobuf: invalid field number 0");
    const field: Field | undefined = message.fieldByNumber.get(fieldNo);

    if (!field) {
      r.skip(wire);
      (out[UNKNOWN] as Uint8Array[] | undefined ?? (out[UNKNOWN] = [] as Uint8Array[]) as Uint8Array[]).push(
        r.buf.slice(tagStart, r.pos),
      );
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
        if (n === 1) k = readSingle(sub, field.map.key.type);
        else if (n === 2) v = readSingle(sub, field.map.value.type);
        else sub.skip(w);
      }
      const obj = (out[field.jsonName] as Record<string, unknown>) ?? (out[field.jsonName] = {});
      obj[String(k)] = v;
      continue;
    }

    if (field.repeated) {
      const arr = (out[field.jsonName] as unknown[]) ?? (out[field.jsonName] = []);
      const packable = field.type.kind === "enum" || (field.type.kind === "scalar" && field.type.scalar !== "string" && field.type.scalar !== "bytes");
      if (wire === WIRE_LEN && packable) {
        const sub = r.fork();
        while (!sub.eof()) {
          arr.push(field.type.kind === "enum" ? readEnum(sub, field.type.enum) : readScalar(sub, (field.type as { scalar: ScalarType }).scalar));
        }
      } else {
        arr.push(readSingle(r, field.type));
      }
      continue;
    }

    // singular
    if (wire !== expectedWire(field.type) && !(wire === WIRE_LEN && field.type.kind === "message")) {
      // wire type doesn't match the schema — preserve as unknown rather than misread
      r.skip(wire);
      (out[UNKNOWN] as Uint8Array[] | undefined ?? (out[UNKNOWN] = [] as Uint8Array[]) as Uint8Array[]).push(
        r.buf.slice(tagStart, r.pos),
      );
      continue;
    }
    if (field.oneofIndex >= 0) {
      for (const num of message.oneofs[field.oneofIndex]!.fieldNumbers) {
        const other = message.fieldByNumber.get(num)!;
        if (other.jsonName !== field.jsonName) delete out[other.jsonName];
      }
    }
    if (field.type.kind === "message") {
      // Repeated occurrences of a singular message field merge (proto spec).
      const existing = out[field.jsonName];
      const into = existing && typeof existing === "object" ? (existing as Record<string, unknown>) : undefined;
      out[field.jsonName] = decode(field.type.message, r.fork(), into);
    } else {
      out[field.jsonName] = readSingle(r, field.type);
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
