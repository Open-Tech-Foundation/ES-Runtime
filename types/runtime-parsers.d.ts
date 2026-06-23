declare module "runtime:parsers" {
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

  export namespace Protobuf {
    /**
     * A dynamically compiled Protobuf Schema.
     */
    export class Schema {
      /**
       * Compiles a Protobuf schema from a string.
       * @param protoStr The content of a `.proto` file defining the schema.
       */
      constructor(protoStr: string);

      /**
       * Parses a Protobuf payload into a JavaScript object based on a message definition.
       * Uses the canonical proto3 JSON mapping: 64-bit integer fields
       * (`int64`/`uint64`/`fixed64`) are returned as strings, and enum fields as
       * their value names.
       * @param messageName The fully-qualified name of the message type (e.g. "package.Message").
       * @param payload The raw Protobuf bytes.
       * @returns The parsed JavaScript object.
       */
      parse(messageName: string, payload: Uint8Array): any;

      /**
       * Builds a raw Protobuf byte payload from a JavaScript object. Accepts the
       * proto3 JSON mapping (64-bit ints as strings or numbers, enums as names or
       * numbers).
       * @param messageName The fully-qualified name of the message type.
       * @param obj The JavaScript object containing the message data.
       * @returns The serialized Protobuf bytes.
       */
      build(messageName: string, obj: any): Uint8Array;

      /**
       * Releases the compiled schema held by the host. Idempotent. The schema is
       * also disposed automatically when declared with `using`.
       */
      free(): void;
      [Symbol.dispose](): void;
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
}
