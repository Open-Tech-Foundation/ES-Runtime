// Load schemas from a compiled FileDescriptorSet (protoc --descriptor_set_out)
// rather than .proto source. A descriptor set is itself protobuf, so we decode
// it with an embedded subset of google/protobuf/descriptor.proto and map each
// FileDescriptorProto onto the same AST the .proto parser produces — then the
// usual link() builds the registry. Only proto3 and editions 2023/2024 are
// accepted (matching the text parser); proto2 descriptors are rejected.
import type { AstEnum, AstField, AstMessage, AstOneof, ParsedFile } from "./parser.js";
import { parseProto } from "./parser.js";
import type { FeatureSet } from "./features.js";
import { type Registry, link } from "./link.js";
import { decode } from "./decode.js";
import { Reader } from "./reader.js";
import { WKT } from "./wkt.js";

// Only the fields we read, with their canonical descriptor.proto field numbers.
// Enum-typed fields are declared as enums so decode yields the value-name string.
const DESCRIPTOR_PROTO = `
syntax = "proto3";
package google.protobuf;

message FileDescriptorSet { repeated FileDescriptorProto file = 1; }

message FileDescriptorProto {
  string name = 1;
  string package = 2;
  repeated string dependency = 3;
  repeated DescriptorProto message_type = 4;
  repeated EnumDescriptorProto enum_type = 5;
  FileOptions options = 8;
  string syntax = 12;
  int32 edition = 14;
}

message DescriptorProto {
  string name = 1;
  repeated FieldDescriptorProto field = 2;
  repeated DescriptorProto nested_type = 3;
  repeated EnumDescriptorProto enum_type = 4;
  MessageOptions options = 7;
  repeated OneofDescriptorProto oneof_decl = 8;
}

message FieldDescriptorProto {
  enum Label { LABEL_UNKNOWN = 0; LABEL_OPTIONAL = 1; LABEL_REQUIRED = 2; LABEL_REPEATED = 3; }
  enum Type {
    TYPE_UNKNOWN = 0; TYPE_DOUBLE = 1; TYPE_FLOAT = 2; TYPE_INT64 = 3; TYPE_UINT64 = 4;
    TYPE_INT32 = 5; TYPE_FIXED64 = 6; TYPE_FIXED32 = 7; TYPE_BOOL = 8; TYPE_STRING = 9;
    TYPE_GROUP = 10; TYPE_MESSAGE = 11; TYPE_BYTES = 12; TYPE_UINT32 = 13; TYPE_ENUM = 14;
    TYPE_SFIXED32 = 15; TYPE_SFIXED64 = 16; TYPE_SINT32 = 17; TYPE_SINT64 = 18;
  }
  string name = 1;
  int32 number = 3;
  Label label = 4;
  Type type = 5;
  string type_name = 6;
  FieldOptions options = 8;
  int32 oneof_index = 9;
  string json_name = 10;
  bool proto3_optional = 17;
}

message OneofDescriptorProto { string name = 1; OneofOptions options = 2; }
message EnumDescriptorProto { string name = 1; repeated EnumValueDescriptorProto value = 2; EnumOptions options = 3; }
message EnumValueDescriptorProto { string name = 1; int32 number = 2; }

message FeatureSet {
  enum FieldPresence { FIELD_PRESENCE_UNKNOWN = 0; EXPLICIT = 1; IMPLICIT = 2; LEGACY_REQUIRED = 3; }
  enum EnumType { ENUM_TYPE_UNKNOWN = 0; OPEN = 1; CLOSED = 2; }
  enum RepeatedFieldEncoding { REPEATED_FIELD_ENCODING_UNKNOWN = 0; PACKED = 1; EXPANDED = 2; }
  enum MessageEncoding { MESSAGE_ENCODING_UNKNOWN = 0; LENGTH_PREFIXED = 1; DELIMITED = 2; }
  FieldPresence field_presence = 1;
  EnumType enum_type = 2;
  RepeatedFieldEncoding repeated_field_encoding = 3;
  MessageEncoding message_encoding = 5;
}

message FileOptions { FeatureSet features = 50; }
message MessageOptions { bool map_entry = 7; FeatureSet features = 12; }
message FieldOptions { bool packed = 2; FeatureSet features = 21; }
message EnumOptions { FeatureSet features = 7; }
message OneofOptions { FeatureSet features = 1; }
`;

const SCALAR_BY_TYPE: Record<string, string> = {
  TYPE_DOUBLE: "double", TYPE_FLOAT: "float", TYPE_INT64: "int64", TYPE_UINT64: "uint64",
  TYPE_INT32: "int32", TYPE_FIXED64: "fixed64", TYPE_FIXED32: "fixed32", TYPE_BOOL: "bool",
  TYPE_STRING: "string", TYPE_BYTES: "bytes", TYPE_UINT32: "uint32", TYPE_SFIXED32: "sfixed32",
  TYPE_SFIXED64: "sfixed64", TYPE_SINT32: "sint32", TYPE_SINT64: "sint64",
};

/* eslint-disable @typescript-eslint/no-explicit-any */
type Obj = Record<string, any>;

let descRegistry: Registry | null = null;
function descriptorRegistry(): Registry {
  return (descRegistry ??= link([parseProto(DESCRIPTOR_PROTO)]));
}

function mapFeatures(f: Obj | undefined): FeatureSet {
  const out: FeatureSet = {};
  if (!f) return out;
  if (f.fieldPresence && f.fieldPresence !== "FIELD_PRESENCE_UNKNOWN") out.fieldPresence = f.fieldPresence;
  if (f.enumType && f.enumType !== "ENUM_TYPE_UNKNOWN") out.enumType = f.enumType;
  if (f.repeatedFieldEncoding && f.repeatedFieldEncoding !== "REPEATED_FIELD_ENCODING_UNKNOWN") out.repeatedEncoding = f.repeatedFieldEncoding;
  if (f.messageEncoding && f.messageEncoding !== "MESSAGE_ENCODING_UNKNOWN") out.messageEncoding = f.messageEncoding;
  return out;
}

function typeRef(f: Obj): string {
  if (f.type === "TYPE_MESSAGE" || f.type === "TYPE_ENUM" || f.type === "TYPE_GROUP") return f.typeName as string;
  const scalar = typeof f.type === "string" ? SCALAR_BY_TYPE[f.type] : undefined;
  if (!scalar) throw new Error(`protobuf: unsupported field type ${f.type} in descriptor set`);
  return scalar;
}

function mapField(f: Obj, mapEntries: Map<string, { key: string; value: string }>): AstField {
  const repeated = f.label === "LABEL_REPEATED";
  const entryName = typeof f.typeName === "string" ? f.typeName.split(".").pop()! : "";
  if (repeated && f.type === "TYPE_MESSAGE" && mapEntries.has(entryName)) {
    const e = mapEntries.get(entryName)!;
    return {
      label: "singular", typeName: "", name: f.name, number: f.number,
      jsonName: f.jsonName, features: mapFeatures(f.options?.features), map: e,
    };
  }
  const label: AstField["label"] = repeated ? "repeated" : f.proto3Optional ? "optional" : "singular";
  return {
    label, typeName: typeRef(f), name: f.name, number: f.number,
    jsonName: f.jsonName, packedOption: f.options?.packed, features: mapFeatures(f.options?.features),
  };
}

function mapEnum(e: Obj): AstEnum {
  return {
    name: e.name,
    values: ((e.value as Obj[]) ?? []).map((v) => ({ name: v.name, number: v.number ?? 0 })),
    features: mapFeatures(e.options?.features),
  };
}

function mapMessage(d: Obj): AstMessage {
  const nested = (d.nestedType as Obj[]) ?? [];
  const mapEntries = new Map<string, { key: string; value: string }>();
  for (const nt of nested) {
    if (nt.options?.mapEntry) {
      const field = (nt.field as Obj[]) ?? [];
      const k = field.find((x) => x.number === 1)!;
      const v = field.find((x) => x.number === 2)!;
      mapEntries.set(nt.name, { key: typeRef(k), value: typeRef(v) });
    }
  }

  const oneofDecl = (d.oneofDecl as Obj[]) ?? [];
  const realOneof = new Map<number, AstField[]>();
  const fields: AstField[] = [];
  for (const f of (d.field as Obj[]) ?? []) {
    const af = mapField(f, mapEntries);
    if (!f.proto3Optional && f.oneofIndex != null) {
      let arr = realOneof.get(f.oneofIndex);
      if (!arr) realOneof.set(f.oneofIndex, (arr = []));
      arr.push(af);
    } else {
      fields.push(af);
    }
  }
  const oneofs: AstOneof[] = [...realOneof.keys()].sort((a, b) => a - b)
    .map((idx) => ({ name: oneofDecl[idx]?.name ?? `oneof_${idx}`, fields: realOneof.get(idx)! }));

  return {
    name: d.name,
    fields,
    oneofs,
    messages: nested.filter((nt) => !nt.options?.mapEntry).map(mapMessage),
    enums: ((d.enumType as Obj[]) ?? []).map(mapEnum),
    features: mapFeatures(d.options?.features),
  };
}

function mapSyntax(syntax: string | undefined, edition: number | undefined): ParsedFile["syntax"] {
  if (syntax === "editions") {
    if (edition === 1000) return "2023";
    if (edition === 1001) return "2024";
    throw new Error(`protobuf: descriptor set uses an unsupported edition (${edition ?? "unknown"})`);
  }
  if (syntax === "proto3") return "proto3";
  throw new Error(`protobuf: descriptor set uses ${syntax ?? "proto2"} — only proto3 and editions 2023/2024 are supported`);
}

function mapFile(fd: Obj): ParsedFile {
  return {
    syntax: mapSyntax(fd.syntax, fd.edition),
    package: fd.package ?? "",
    imports: (fd.dependency as string[]) ?? [],
    features: mapFeatures(fd.options?.features),
    messages: ((fd.messageType as Obj[]) ?? []).map(mapMessage),
    enums: ((fd.enumType as Obj[]) ?? []).map(mapEnum),
  };
}

/** Decodes a serialized FileDescriptorSet into the parsed-file model that
 *  link() consumes. Well-known-type files absent from the set (i.e. when
 *  `--include_imports` was not used) are supplied from the embedded sources. */
export function parseDescriptorSet(bytes: Uint8Array): ParsedFile[] {
  const reg = descriptorRegistry();
  const setType = reg.messages.get("google.protobuf.FileDescriptorSet")!;
  const fds = decode(setType, new Reader(bytes)) as Obj;
  const files = (fds.file as Obj[]) ?? [];

  const present = new Set(files.map((f) => f.name as string));
  const parsed = files.map(mapFile);
  for (const [name, src] of Object.entries(WKT)) {
    if (!present.has(name)) parsed.push(parseProto(src));
  }
  return parsed;
}
