declare module "runtime:serialization" {
  export interface ValidationOptions {
    detailed?: boolean;
  }

  export interface ValidationDetailedResult {
    valid: boolean;
    error?: string;
  }

  export namespace XML {
    /**
     * Validates an XML string.
     * @param xml The XML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(xml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /**
     * Parses an XML string into a JavaScript object.
     * @param xml The XML string to parse.
     * @returns The parsed JavaScript object.
     */
    export function parse(xml: string): any;

    /**
     * Builds an XML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @returns The serialized XML string.
     */
    export function build(obj: any): string;

    export class DecoderStream extends TransformStream<string | Uint8Array, any> {
      constructor();
    }
  }

  export namespace YAML {
    /**
     * Validates a YAML string.
     * @param yaml The YAML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(yaml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /**
     * Parses a YAML string into a JavaScript object.
     * @param yaml The YAML string to parse.
     * @returns The parsed JavaScript object.
     */
    export function parse(yaml: string): any;

    /**
     * Builds a YAML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @returns The serialized YAML string.
     */
    export function build(obj: any): string;
  }

  export namespace TOML {
    /**
     * Validates a TOML string.
     * @param toml The TOML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(toml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /**
     * Parses a TOML string into a JavaScript object.
     * @param toml The TOML string to parse.
     * @returns The parsed JavaScript object.
     */
    export function parse(toml: string): any;

    /**
     * Builds a TOML string from a JavaScript object.
     * @param obj The JavaScript object to serialize. The root must be an object (TOML table).
     * @returns The serialized TOML string.
     */
    export function build(obj: any): string;
  }

  export namespace MessagePack {
    /**
     * Validates a MessagePack byte array.
     * @param msgpack The MessagePack byte array to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(msgpack: Uint8Array, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /**
     * Decodes a MessagePack byte array into a JavaScript object.
     * @param msgpack The MessagePack byte array to parse.
     * @returns The parsed JavaScript object.
     */
    export function decode(msgpack: Uint8Array): any;

    /**
     * Encodes a JavaScript object to a MessagePack byte array.
     * @param obj The JavaScript object to serialize.
     * @returns The serialized MessagePack byte array.
     */
    export function encode(obj: any): Uint8Array;
  }

  export interface JSONLParseError {
    line: number;
    raw: string;
    cause: Error;
  }

  export interface JSONLDecoderOptions {
    skipInvalid?: boolean;
  }

  export namespace JSONL {
    export class DecoderStream<T = any> extends TransformStream<string | Uint8Array, T> {
      constructor(options?: JSONLDecoderOptions);
      onError(callback: (error: JSONLParseError) => void): void;
    }

    export class EncoderStream<T = any> extends TransformStream<T, string> {
      constructor();
      pipeTo(destination: WritableStream<string | Uint8Array>, options?: StreamPipeOptions): Promise<void>;
      write(chunk: T): Promise<void>;
      close(): Promise<void>;
    }
  }

  export interface ProtobufSchemaOptions {
    /** Entry filename when the schema is given as a file map. */
    entry?: string;
  }

    /**
     * A canonical proto3-JSON value: 64-bit integers and `bytes` are strings
     * (base64 for `bytes`), enums are their value-name string, and the
     * well-known types take their special forms (Timestamp/Duration as strings,
     * wrappers as bare values, Struct/Value/ListValue as native JSON, Any with
     * an `@type` member, FieldMask as a comma path string, Empty as `{}`).
     */
    export type JsonValue =
      | null | boolean | number | string | JsonValue[] | { [k: string]: JsonValue };

  export namespace Protobuf {
    /**
     * A protobuf schema compiled at runtime from `.proto` source. proto2,
     * proto3, and editions 2023/2024 are supported; proto2 `group`s lower to
     * delimited message fields, while `extend`/extensions are unsupported (such
     * fields round-trip losslessly as preserved unknown fields). Decoding is
     * reflective — no codegen.
     *
     * Decoded value shape: camelCase field names; 64-bit integer fields
     * (`int64`/`uint64`/`sint64`/`fixed64`/`sfixed64`) as **BigInt**; enums as
     * their value-name string (unknown numbers kept as numbers); `bytes` as
     * `Uint8Array`; maps as plain objects; nested messages as plain objects.
     * Fields absent on the wire are omitted from the result.
     */
    export class Schema {
      /**
       * @param proto A single `.proto` source string, or a map of
       *   filename → source for multi-file schemas with `import`s. The
       *   `google/protobuf/*` well-known types resolve without being provided.
       */
      constructor(proto: string | Record<string, string>, options?: ProtobufSchemaOptions);

      /**
       * Builds a Schema from a compiled `FileDescriptorSet` (protoc
       * `--descriptor_set_out`) rather than `.proto` source. Use
       * `--include_imports` so referenced types are present; the
       * `google/protobuf/*` well-known types are otherwise supplied from the
       * embedded sources. proto2 (including descriptor sets with an unset
       * `syntax` field), proto3, and editions 2023/2024 are accepted.
       */
      static fromDescriptorSet(descriptorSet: Uint8Array): Schema;

      /** Decodes binary protobuf for the fully-qualified `messageName`. */
      decode(messageName: string, bytes: Uint8Array): Record<string, unknown>;

      /** Encodes `value` as binary protobuf for the fully-qualified `messageName`. */
      encode(messageName: string, value: Record<string, unknown>): Uint8Array;

      /**
       * Encodes `value` as a single length-delimited message — a varint length
       * prefix followed by the encoded bytes (the `writeDelimitedTo` framing).
       * Concatenate results to write a stream of messages.
       */
      encodeDelimited(messageName: string, value: Record<string, unknown>): Uint8Array;

      /**
       * Converts a decoded value (the `decode` shape) to its canonical
       * proto3-JSON representation for the fully-qualified `messageName`.
       */
      toJson(messageName: string, value: Record<string, unknown>): JsonValue;

      /**
       * Parses canonical proto3-JSON into the decoded value shape (ready for
       * `encode`) for the fully-qualified `messageName`. Parsing is strict;
       * pass `{ ignoreUnknownFields: true }` to drop unrecognized fields and
       * unknown enum-name strings instead of throwing.
       */
      fromJson(
        messageName: string,
        json: JsonValue,
        options?: { ignoreUnknownFields?: boolean },
      ): Record<string, unknown>;

      /**
       * Streams the elements of a repeated message field (`fieldName`) from a
       * chunked byte source — a `ReadableStream`, async iterable, or sync
       * iterable of `Uint8Array`. Each element is decoded and yielded as it is
       * fully buffered; the outer message's other fields are skipped, so a
       * large collection is processed without materializing the whole array.
       */
      decodeStream(
        messageName: string,
        fieldName: string,
        source: ReadableStream<Uint8Array> | AsyncIterable<Uint8Array> | Iterable<Uint8Array> | Uint8Array,
      ): AsyncGenerator<Record<string, unknown>>;

      /**
       * Streams the messages of a length-delimited stream (varint-length-
       * prefixed, the `writeDelimitedTo` framing) from a chunked byte `source` —
       * a `ReadableStream`, async/sync iterable of `Uint8Array`, or a single
       * `Uint8Array` — decoding and yielding each message in turn.
       */
      decodeDelimited(
        messageName: string,
        source: ReadableStream<Uint8Array> | AsyncIterable<Uint8Array> | Iterable<Uint8Array> | Uint8Array,
      ): AsyncGenerator<Record<string, unknown>>;
    }
  }
}
