// Links parsed .proto files into the resolved descriptor model: registers all
// types, links field type references, lowers maps, and resolves editions/proto3
// features into concrete presence/packed/closed flags.
import type { AstEnum, AstField, AstMessage, ParsedFile } from "./parser.js";
import { Parser } from "./parser.js";
import {
  type EnumType, type Field, type FieldType, type MessageType,
  type ScalarType,
} from "./descriptor.js";
import { type FeatureSet, baseFeatures, mergeFeatures } from "./features.js";

export interface Registry {
  messages: Map<string, MessageType>;
  enums: Map<string, EnumType>;
  get(fullName: string): MessageType | EnumType | undefined;
}

const PACKABLE_SCALARS: ReadonlySet<string> = new Set([
  "double", "float", "int32", "int64", "uint32", "uint64", "sint32", "sint64",
  "fixed32", "fixed64", "sfixed32", "sfixed64", "bool",
]);

function camelCase(s: string): string {
  let out = "";
  let up = false;
  for (const c of s) {
    if (c === "_") up = true;
    else { out += up ? c.toUpperCase() : c; up = false; }
  }
  return out;
}

function qualify(scope: string, name: string): string {
  return scope ? scope + "." + name : name;
}

export function link(files: ParsedFile[]): Registry {
  const messages = new Map<string, MessageType>();
  const enums = new Map<string, EnumType>();
  interface Job { ast: AstMessage; desc: MessageType; scope: string; inherited: Required<FeatureSet>; }
  const jobs: Job[] = [];

  function registerEnum(ast: AstEnum, scope: string, inherited: Required<FeatureSet>): void {
    const fullName = qualify(scope, ast.name);
    const closed = mergeFeatures(inherited, ast.features).enumType === "CLOSED";
    const byNumber = new Map<number, string>();
    const byName = new Map<string, number>();
    for (const v of ast.values) {
      if (!byNumber.has(v.number)) byNumber.set(v.number, v.name);
      byName.set(v.name, v.number);
    }
    enums.set(fullName, { name: ast.name, fullName, values: ast.values, byNumber, byName, closed });
  }

  function registerMessage(ast: AstMessage, scope: string, inherited: Required<FeatureSet>): void {
    const fullName = qualify(scope, ast.name);
    const msgFeat = mergeFeatures(inherited, ast.features);
    const desc: MessageType = {
      name: ast.name, fullName, fields: [], fieldByNumber: new Map(), oneofs: [], isMapEntry: false,
    };
    messages.set(fullName, desc);
    jobs.push({ ast, desc, scope: fullName, inherited: msgFeat });
    for (const e of ast.enums) registerEnum(e, fullName, msgFeat);
    for (const m of ast.messages) registerMessage(m, fullName, msgFeat);
  }

  // Pass 1: register every type so references resolve in pass 2.
  for (const file of files) {
    const fileFeat = mergeFeatures(baseFeatures(file.syntax), file.features);
    for (const e of file.enums) registerEnum(e, file.package, fileFeat);
    for (const m of file.messages) registerMessage(m, file.package, fileFeat);
  }

  const registry: Registry = {
    messages, enums,
    get(n) { return messages.get(n) ?? enums.get(n); },
  };

  // Resolve a (possibly relative / leading-dot) type name against a scope.
  function resolveType(ref: string, scope: string): FieldType {
    if (Parser.isScalar(ref)) return { kind: "scalar", scalar: ref as ScalarType };
    const abs = ref.startsWith(".");
    const bare = abs ? ref.slice(1) : ref;
    const tryNames: string[] = [];
    if (abs) {
      tryNames.push(bare);
    } else {
      const parts = scope ? scope.split(".") : [];
      for (let i = parts.length; i >= 0; i--) {
        tryNames.push([...parts.slice(0, i), bare].join("."));
      }
    }
    for (const n of tryNames) {
      const m = messages.get(n);
      if (m) return { kind: "message", message: m };
      const e = enums.get(n);
      if (e) return { kind: "enum", enum: e };
    }
    throw new Error(`protobuf: unknown type "${ref}" referenced from "${scope}"`);
  }

  function resolveField(ast: AstField, scope: string, inherited: Required<FeatureSet>, oneofIndex: number): Field {
    const feat = mergeFeatures(inherited, ast.features);
    const jsonName = ast.jsonName ?? camelCase(ast.name);

    if (ast.map) {
      const keyType = resolveType(ast.map.key, scope);
      const valueType = resolveType(ast.map.value, scope);
      const key: Field = { name: "key", jsonName: "key", number: 1, repeated: false, explicitPresence: false, packed: false, delimited: false, type: keyType, oneofIndex: -1 };
      const value: Field = { name: "value", jsonName: "value", number: 2, repeated: false, explicitPresence: false, packed: false, delimited: false, type: valueType, oneofIndex: -1 };
      return { name: ast.name, jsonName, number: ast.number, repeated: false, explicitPresence: false, packed: false, delimited: false, type: valueType, oneofIndex: -1, map: { key, value } };
    }

    const type = resolveType(ast.typeName, scope);
    const repeated = ast.label === "repeated";

    let explicitPresence = false;
    let packed = false;
    if (repeated) {
      const packable = type.kind === "enum" || (type.kind === "scalar" && PACKABLE_SCALARS.has(type.scalar));
      packed = packable && (ast.packedOption ?? feat.repeatedEncoding === "PACKED");
    } else if (type.kind === "message") {
      explicitPresence = true; // messages always have presence
    } else if (ast.label === "optional" || oneofIndex >= 0) {
      explicitPresence = true;
    } else {
      explicitPresence = feat.fieldPresence === "EXPLICIT";
    }

    // Group (delimited) encoding applies to message fields only.
    const delimited = type.kind === "message" && feat.messageEncoding === "DELIMITED";

    return { name: ast.name, jsonName, number: ast.number, repeated, explicitPresence, packed, delimited, type, oneofIndex };
  }

  // Pass 2: resolve each message's fields and oneofs.
  for (const { ast, desc, scope, inherited } of jobs) {
    const add = (f: Field) => { desc.fields.push(f); desc.fieldByNumber.set(f.number, f); };

    for (const af of ast.fields) add(resolveField(af, scope, inherited, -1));

    ast.oneofs.forEach((oneof, idx) => {
      const oneofIndex = desc.oneofs.length;
      const fieldNumbers: number[] = [];
      for (const af of oneof.fields) {
        const f = resolveField(af, scope, inherited, oneofIndex);
        add(f);
        fieldNumbers.push(f.number);
      }
      desc.oneofs.push({ name: oneof.name, fieldNumbers });
      void idx;
    });
  }

  return registry;
}
