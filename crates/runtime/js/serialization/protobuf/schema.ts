// Public Protobuf API. Compiles a .proto schema (a single source string, or a
// map of filename → source for multi-file schemas with imports) at runtime and
// decodes/encodes messages reflectively against it.
import { type ParsedFile, parseProto } from "./parser.js";
import { type Registry, link } from "./link.js";
import { decode } from "./decode.js";
import { encode } from "./encode.js";
import { type FromJsonOptions, type JsonValue, messageFromJson, messageToJson } from "./json.js";
import { type StreamSource, decodeDelimitedStream, decodeStream } from "./stream.js";
import { parseDescriptorSet } from "./descriptor_set.js";
import { Reader } from "./reader.js";
import { Writer } from "./writer.js";
import { WKT } from "./wkt.js";

export interface SchemaOptions {
  /** Entry filename when `proto` is a file map (defaults to all files). */
  entry?: string;
}

export class Schema {
  private registry: Registry;

  constructor(proto: string | Record<string, string>, _opts: SchemaOptions = {}) {
    const sources: Record<string, string> =
      typeof proto === "string" ? { "__main__.proto": proto } : { ...proto };

    const parsed: ParsedFile[] = [];
    const seen = new Set<string>();
    const toParse = Object.keys(sources);

    while (toParse.length) {
      const name = toParse.shift()!;
      if (seen.has(name)) continue;
      seen.add(name);
      const src = sources[name] ?? WKT[name];
      if (src == null) throw new Error(`protobuf: cannot resolve import "${name}"`);
      const pf = parseProto(src);
      parsed.push(pf);
      for (const imp of pf.imports) if (!seen.has(imp)) toParse.push(imp);
    }

    this.registry = link(parsed);
  }

  /** Builds a Schema from a compiled `FileDescriptorSet` (protoc
   *  `--descriptor_set_out`) instead of `.proto` source. Use `--include_imports`
   *  so referenced types are present; the `google/protobuf/*` well-known types
   *  are otherwise supplied from the embedded sources. */
  static fromDescriptorSet(descriptorSet: Uint8Array): Schema {
    const schema = Object.create(Schema.prototype) as Schema;
    (schema as unknown as { registry: Registry }).registry = link(parseDescriptorSet(descriptorSet));
    return schema;
  }

  /** Decodes binary protobuf bytes for the fully-qualified `messageName`. */
  decode(messageName: string, bytes: Uint8Array): Record<string, unknown> {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    return decode(m, new Reader(bytes));
  }

  /** Encodes `value` as binary protobuf for the fully-qualified `messageName`. */
  encode(messageName: string, value: Record<string, unknown>): Uint8Array {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    const w = new Writer();
    encode(m, value, w);
    return w.finish();
  }

  /** Encodes `value` as a single length-delimited message — a varint length
   *  prefix followed by the encoded bytes (the `writeDelimitedTo` framing).
   *  Concatenate the results to write a stream of messages. */
  encodeDelimited(messageName: string, value: Record<string, unknown>): Uint8Array {
    const body = this.encode(messageName, value);
    const w = new Writer();
    w.uint32(body.length);
    w.raw(body);
    return w.finish();
  }

  /** Converts a decoded value to its canonical proto3-JSON representation. */
  toJson(messageName: string, value: Record<string, unknown>): JsonValue {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    return messageToJson(m, value, this.registry);
  }

  /** Parses canonical proto3-JSON into the decoded value shape (`encode`-ready). */
  fromJson(messageName: string, json: JsonValue, options: FromJsonOptions = {}): Record<string, unknown> {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    return messageFromJson(m, json, this.registry, options);
  }

  /** Streams the elements of a repeated message field from a chunked byte
   *  `source` (a ReadableStream or async/sync iterable of `Uint8Array`),
   *  decoding each element as it arrives and skipping the other fields. */
  decodeStream(messageName: string, fieldName: string, source: StreamSource): AsyncGenerator<Record<string, unknown>> {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    const field = m.fields.find((f) => f.jsonName === fieldName || f.name === fieldName);
    if (!field) throw new Error(`protobuf: unknown field "${fieldName}" in ${messageName}`);
    if (!field.repeated || field.type.kind !== "message") {
      throw new Error(`protobuf: decodeStream requires a repeated message field; "${fieldName}" is not one`);
    }
    if (field.delimited) {
      throw new Error(`protobuf: decodeStream does not support delimited (group) fields`);
    }
    return decodeStream(field, source);
  }

  /** Streams the messages of a length-delimited stream (varint-length-prefixed,
   *  the `writeDelimitedTo` framing) from a chunked byte `source`, decoding and
   *  yielding each message in turn. */
  decodeDelimited(messageName: string, source: StreamSource): AsyncGenerator<Record<string, unknown>> {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    return decodeDelimitedStream(m, source);
  }
}

export const Protobuf = { Schema };
