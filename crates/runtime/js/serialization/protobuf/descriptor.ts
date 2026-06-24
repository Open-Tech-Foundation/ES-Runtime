// Normalized, resolved schema model that the codec walks. Produced by parser.ts
// (which links type references and resolves editions/proto3 features into the
// concrete flags below), consumed by decode.ts / encode.ts.

export type ScalarType =
  | "double" | "float"
  | "int32" | "int64" | "uint32" | "uint64"
  | "sint32" | "sint64"
  | "fixed32" | "fixed64" | "sfixed32" | "sfixed64"
  | "bool" | "string" | "bytes";

/** Scalar types decoded/encoded as BigInt (the 64-bit family). */
export const BIGINT_SCALARS: ReadonlySet<ScalarType> = new Set([
  "int64", "uint64", "sint64", "fixed64", "sfixed64",
]);

export interface EnumType {
  name: string;
  fullName: string;
  values: { name: string; number: number }[];
  byNumber: Map<number, string>;
  byName: Map<string, number>;
  /** Closed enums (proto2/editions CLOSED) reject unknown numbers into unknown
   *  fields; open enums (proto3 default) keep them. */
  closed: boolean;
}

export type FieldType =
  | { kind: "scalar"; scalar: ScalarType }
  | { kind: "enum"; enum: EnumType }
  | { kind: "message"; message: MessageType };

export interface Field {
  name: string;
  jsonName: string;
  number: number;
  repeated: boolean;
  /** Resolved field presence for singular fields. Explicit = serialize even at
   *  default and track presence; implicit (proto3 default) = omit at default. */
  explicitPresence: boolean;
  /** Resolved packed encoding for repeated scalar/enum fields. */
  packed: boolean;
  /** Group (delimited) wire encoding for a message field — editions
   *  `features.message_encoding = DELIMITED`. */
  delimited: boolean;
  type: FieldType;
  /** Index into message.oneofs, or -1. Synthetic oneofs (proto3 `optional`) are
   *  flattened: the field stays a normal optional field, oneofIndex = -1. */
  oneofIndex: number;
  /** Present iff this field is a `map<K,V>`. */
  map?: { key: Field; value: Field };
}

export interface Oneof {
  name: string;
  fieldNumbers: number[];
}

export interface MessageType {
  name: string;
  fullName: string;
  fields: Field[];
  fieldByNumber: Map<number, Field>;
  oneofs: Oneof[];
  isMapEntry: boolean;
}

/** The default (zero) value for an implicit-presence scalar/enum field, used to
 *  decide whether to omit it on encode and what an absent field means. */
export function scalarDefault(t: ScalarType): unknown {
  switch (t) {
    case "double": case "float":
    case "int32": case "uint32": case "sint32":
    case "fixed32": case "sfixed32":
      return 0;
    case "int64": case "uint64": case "sint64":
    case "fixed64": case "sfixed64":
      return 0n;
    case "bool":
      return false;
    case "string":
      return "";
    case "bytes":
      return new Uint8Array(0);
  }
}
