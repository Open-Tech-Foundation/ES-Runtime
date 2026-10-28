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

  export interface CSVOptions {
    /** Field separator; a single byte. Default ",". */
    delimiter?: string;
    /** Quote character; a single byte. Default "\"". */
    quote?: string;
    /** Escape character inside a quoted field. Default is RFC 4180 doubling. */
    escape?: string;
    /**
     * `false` (default) returns arrays of fields; `true` keys objects by the
     * first row; an array of names keys every row and treats the first row as
     * data.
     */
    header?: boolean | string[];
    /**
     * Trim surrounding whitespace from unquoted fields, and skip spaces and tabs
     * around a quoted one. A tab delimiter is never trimmed. Default false.
     */
    trim?: boolean;
    /** Drop records that are a single empty field. Default false. */
    skipEmptyLines?: boolean;
    /**
     * A single byte, other than the delimiter, the quote or a line break; lines
     * beginning with it are skipped whole. `build` quotes a
     * field that starts with it, so the row is not lost on a round trip.
     */
    comment?: string;
    /**
     * Convert plain decimal fields to numbers and `true`/`false` to booleans.
     * Default false — every field is a string unless asked. `inf`, `nan`, hex,
     * a decimal that overflows `f64`, and an integer past 2^53 - 1 stay strings.
     */
    cast?: boolean;
    /**
     * Allow rows whose field count differs from the header. Off by default, so
     * a ragged row is an error. Missing keys are omitted; extras are dropped.
     */
    relaxColumnCount?: boolean;
    /** Per-field byte cap. Default 16 MiB. */
    maxFieldBytes?: number;
    /** Per-record field cap. Default 1_048_576. */
    maxColumns?: number;
    /** Strip a leading UTF-8 BOM. Default true. */
    bom?: boolean;
    /** Record separator `build` writes. Default "\n". */
    newline?: "\n" | "\r\n" | "\r";
  }

  export interface CSVValidateOptions extends CSVOptions {
    detailed?: boolean;
  }

  export namespace CSV {
    /**
     * Validates CSV text under the same options as `parse`.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(csv: string, options?: CSVValidateOptions): boolean | ValidationDetailedResult;

    /**
     * Parses CSV text. Returns an array of field arrays by default, or an array
     * of objects when `header` is set.
     */
    export function parse(csv: string, options?: CSVOptions): any;

    /**
     * Writes an array of rows (arrays or objects) as CSV text.
     */
    export function build(rows: any, options?: CSVOptions): string;

    /**
     * Decodes a stream of CSV text into rows, honouring the same options as
     * `parse`. A `TransformStream`: `source.pipeThrough(new CSV.DecoderStream())`.
     * Quoted fields may span chunks (a quoted newline, a doubled quote, a CRLF
     * split across the boundary), so the scanner state lives in the host.
     */
    export class DecoderStream<T = any> extends TransformStream<string | Uint8Array, T> {
      constructor(options?: CSVOptions);
    }

    /**
     * Encodes a stream of rows (one per chunk) into CSV text, with the same
     * shape rules as `build`: the first row fixes whether rows are arrays or
     * objects, and an object stream infers its header from the first object.
     */
    export class EncoderStream<T = any> extends TransformStream<T, string> {
      constructor(options?: CSVOptions);
      pipeTo(destination: WritableStream<string | Uint8Array>, options?: StreamPipeOptions): Promise<void>;
      write(chunk: T): Promise<void>;
      close(): Promise<void>;
    }
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

  export namespace CBOR {
    /**
     * Validates a CBOR byte array, agreeing with `decode`: a document it would
     * refuse (a tag, a trailing byte, a malformed value) is not valid.
     * @param cbor The CBOR byte array to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    export function validate(cbor: Uint8Array, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /**
     * Decodes a CBOR (RFC 8949) byte array into a JavaScript value. Definite and
     * indefinite-length strings, arrays and maps are accepted; tags are refused,
     * since no JavaScript type corresponds to one.
     * @param cbor The CBOR byte array to decode.
     * @returns The decoded JavaScript value.
     */
    export function decode(cbor: Uint8Array): any;

    /**
     * Encodes a JavaScript value to a CBOR byte array in the deterministic
     * (canonical) form of RFC 8949 §4.2: shortest-form integers and lengths,
     * the shortest float that preserves the value, and bytewise-sorted map
     * keys. A `Date` encodes as its ISO-8601 string, a `Map` as a map, a `Set`
     * as an array, and a typed array as a byte string; a value with no CBOR
     * representation throws rather than being written as `null`.
     * @param obj The JavaScript value to serialize.
     * @returns The serialized CBOR byte array.
     */
    export function encode(obj: any): Uint8Array;
  }

  /**
   * BSON, MongoDB's document format. Encoding and decoding walk live values in
   * the engine; malformed bytes are a `SyntaxError`, a value with no BSON form a
   * `TypeError`, and a value out of range (an int64, nesting past 256 levels) a
   * `RangeError`.
   */
  export namespace BSON {
    /**
     * Encodes a document — an object or a `Map` with string keys — as BSON. A
     * whole `number` in the int32 range writes as int32 and any other `number`
     * as a double; a `bigint` writes as int64; `undefined` fields are omitted;
     * a `RegExp` keeps the `i`, `m`, `s` and `u` flags.
     */
    export function encode(document: object): Uint8Array;

    /**
     * Decodes BSON bytes into a document. int64 reads as `bigint`, datetime as
     * `Date`, binary subtype 0 as `Uint8Array`, subtype 4 as `UUID`, other
     * subtypes as `Binary`, and a regular expression as `BSONRegExp`.
     */
    export function decode(bytes: Uint8Array | ArrayBuffer): Record<string, any>;

    /** Validates BSON bytes, agreeing with `decode`. */
    export function validate(bytes: Uint8Array, options?: ValidationOptions): boolean | ValidationDetailedResult;

    /** A 12-byte object id. */
    export class ObjectId {
      /** A new id, or the one given as 24 hex digits, 12 bytes, or another ObjectId. */
      constructor(id?: string | Uint8Array | ObjectId);
      static isValid(id: unknown): boolean;
      static createFromHexString(hex: string): ObjectId;
      /** An id for `seconds` with every other byte zero, for range queries on `_id`. */
      static createFromTime(seconds: number): ObjectId;
      /** The 12 bytes. */
      readonly id: Uint8Array;
      /** When the id was made, to the second. */
      getTimestamp(): Date;
      equals(other: ObjectId | string | null | undefined): boolean;
      toHexString(): string;
      toString(): string;
      toJSON(): string;
    }

    /** A 128-bit decimal, exactly as the server stores it. */
    export class Decimal128 {
      /** From decimal text, exactly (inexact or malformed text is a `SyntaxError`), or the 16 stored bytes. */
      constructor(value: string | Uint8Array);
      static fromString(text: string): Decimal128;
      /** The 16 bytes, as stored. */
      readonly bytes: Uint8Array;
      toString(): string;
      toJSON(): { $numberDecimal: string };
    }

    /** A UUID, stored as binary subtype 4. */
    export class UUID {
      /** A new random (version 4) UUID, or the one given as text (dashes optional) or 16 bytes. */
      constructor(value?: string | Uint8Array | UUID);
      static isValid(value: unknown): boolean;
      readonly bytes: Uint8Array;
      equals(other: UUID | string | null | undefined): boolean;
      toHexString(includeDashes?: boolean): string;
      toString(): string;
      toJSON(): string;
    }

    /** A server timestamp: seconds (`t`) and an ordinal within the second (`i`). */
    export class Timestamp {
      constructor(value: { t: number; i: number } | bigint);
      readonly t: number;
      readonly i: number;
      toJSON(): { $timestamp: { t: number; i: number } };
    }

    /** Binary data of any subtype. A plain `Uint8Array` is subtype 0 and needs no wrapper. */
    export class Binary {
      static readonly SUBTYPE_DEFAULT: 0;
      static readonly SUBTYPE_FUNCTION: 1;
      static readonly SUBTYPE_BYTE_ARRAY: 2;
      static readonly SUBTYPE_UUID_OLD: 3;
      static readonly SUBTYPE_UUID: 4;
      static readonly SUBTYPE_MD5: 5;
      static readonly SUBTYPE_ENCRYPTED: 6;
      static readonly SUBTYPE_COLUMN: 7;
      static readonly SUBTYPE_SENSITIVE: 8;
      static readonly SUBTYPE_VECTOR: 9;
      static readonly SUBTYPE_USER_DEFINED: 128;
      constructor(buffer?: Uint8Array, subType?: number);
      buffer: Uint8Array;
      subType: number;
      /** A float32 vector (subtype 9). */
      static fromFloat32Array(values: Float32Array | readonly number[]): Binary;
      /** An int8 vector (subtype 9). Numbers outside -128..127 are a `RangeError`. */
      static fromInt8Array(values: Int8Array | readonly number[]): Binary;
      /** A packed-bit vector (subtype 9); the last `padding` bits must be 0. */
      static fromPackedBits(bits: Uint8Array | readonly number[], padding?: number): Binary;
      toFloat32Array(): Float32Array;
      toInt8Array(): Int8Array;
      /** The packed bytes; `buffer[1]` is the padding. */
      toPackedBits(): Uint8Array;
      toJSON(): { $binary: { base64: string; subType: string } };
    }

    /** JavaScript code, with an optional scope. */
    export class Code {
      constructor(code: string | ((...args: any[]) => any), scope?: Record<string, unknown> | null);
      code: string;
      scope: Record<string, unknown> | null;
      toJSON(): { code: string; scope?: Record<string, unknown> };
    }

    /** A regular expression exactly as the server stores it. */
    export class BSONRegExp {
      /** `options` are any of `ilmsux`, stored sorted. */
      constructor(pattern: string, options?: string);
      pattern: string;
      options: string;
      /** The JavaScript `RegExp`; a `SyntaxError` for the `l` and `x` options, which have no JavaScript flag. */
      toRegExp(): RegExp;
      toJSON(): { $regularExpression: { pattern: string; options: string } };
    }

    /** Sorts below every other value. */
    export class MinKey {
      toJSON(): { $minKey: 1 };
    }

    /** Sorts above every other value. */
    export class MaxKey {
      toJSON(): { $maxKey: 1 };
    }

    /** A number that encodes as a double even when it is whole. Reads back as a plain `number`. */
    export class Double {
      constructor(value: number);
      value: number;
      valueOf(): number;
      toJSON(): number;
    }
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

  /** What `JSON.parse` takes as a reviver: called on each value after its children; `undefined` removes the value. */
  export type JSONReviver = (this: any, key: string, value: any) => any;

  /** What `JSON.stringify` takes as a replacer: a function, or the property names to keep. */
  export type JSONReplacer = ((this: any, key: string, value: any) => any) | readonly (string | number)[] | null;

  /**
   * JSON with comments (D191): RFC 8259 JSON plus line (`//`) and block comments and
   * one trailing comma in an object or array, as `tsconfig.json` and VS Code's
   * settings are written. A leading byte order mark is ignored.
   */
  export namespace JSONC {
    /**
     * Parses JSONC into the value `JSON.parse` would build from the same JSON.
     * @throws {SyntaxError} the text is not JSONC; the message ends `at line L column C`.
     * @throws {RangeError} the text nests more than 256 levels deep.
     */
    export function parse(text: string, reviver?: JSONReviver): any;

    /** Writes standard JSON, exactly as `JSON.stringify` does: a comment cannot come from a value. */
    export function stringify(value: unknown, replacer?: JSONReplacer, space?: string | number | null): string;

    /** Checks the text without building a value; agrees with `parse`. */
    export function validate(text: string, options?: ValidationOptions): boolean | ValidationDetailedResult;
  }

  export interface JSON5StringifyOptions {
    replacer?: JSONReplacer;
    space?: string | number | null;
    /** The quote for every string. By default each string takes whichever needs fewer escapes, single on a tie. */
    quote?: "'" | '"';
  }

  /**
   * JSON5 1.0.0 (D191): unquoted keys, single-quoted and multi-line strings,
   * hex, `Infinity` and `NaN`, leading and trailing decimal points, comments
   * and trailing commas.
   */
  export namespace JSON5 {
    /**
     * Parses JSON5. For the JSON it shares, the value is what `JSON.parse` builds.
     * @throws {SyntaxError} the text is not JSON5; the message ends `at line L column C`.
     * @throws {RangeError} the text nests more than 256 levels deep.
     */
    export function parse(text: string, reviver?: JSONReviver): any;

    /**
     * Writes JSON5, as the `json5` package does: ASCII identifier keys unquoted,
     * `Infinity` and `NaN` kept, and a trailing comma after each member when
     * indenting. A value's `toJSON5()` is preferred to its `toJSON()`.
     * Returns `undefined` for a value `JSON.stringify` would skip.
     * @throws {TypeError} a `BigInt`, or a circular value.
     */
    export function stringify(value: unknown, replacer?: JSONReplacer, space?: string | number | null): string;
    export function stringify(value: unknown, options: JSON5StringifyOptions): string;

    /** Checks the text without building a value; agrees with `parse`. */
    export function validate(text: string, options?: ValidationOptions): boolean | ValidationDetailedResult;
  }

  export interface ProtobufSchemaOptions {
    /** Entry filename when the schema is given as a file map. */
    entry?: string;
  }

  export interface ProtobufEncodeOptions {
    /**
     * Accept and skip object keys that match no field, instead of throwing.
     * Off by default, so a misspelled field name cannot quietly encode to a
     * short buffer.
     */
    ignoreUnknownFields?: boolean;
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

      /**
       * Encodes `value` as binary protobuf for the fully-qualified `messageName`.
       *
       * Field names may be spelled either way — `user_name` as it appears in the
       * `.proto`, or the lowerCamelCase `userName` that `decode` produces.
       * Supplying both spellings of one field is an error. A key matching no
       * field throws, so a typo cannot silently encode to a short buffer; pass
       * `{ ignoreUnknownFields: true }` to skip such keys instead.
       */
      encode(
        messageName: string,
        value: Record<string, unknown>,
        options?: ProtobufEncodeOptions,
      ): Uint8Array;

      /**
       * Encodes `value` as a single length-delimited message — a varint length
       * prefix followed by the encoded bytes (the `writeDelimitedTo` framing).
       * Concatenate results to write a stream of messages.
       */
      encodeDelimited(
        messageName: string,
        value: Record<string, unknown>,
        options?: ProtobufEncodeOptions,
      ): Uint8Array;

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
