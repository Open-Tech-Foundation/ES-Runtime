declare module "runtime:parsers" {
  export interface ValidationOptions {
    detailed?: boolean;
  }

  export interface ValidationDetailedResult {
    valid: boolean;
    error?: string;
  }

  export class XMLValidator {
    /**
     * Validates an XML string.
     * @param xml The XML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    static validate(xml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;
  }

  export class XMLParser {
    /**
     * Parses an XML string into a JavaScript object.
     * @param xml The XML string to parse.
     * @returns The parsed JavaScript object.
     */
    static parse(xml: string): any;
  }

  export class XMLBuilder {
    /**
     * Builds an XML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @returns The serialized XML string.
     */
    static build(obj: any): string;
  }

  export class XMLDecoderStream extends TransformStream<string | Uint8Array, any> {
    constructor();
  }

  export class YAMLValidator {
    /**
     * Validates a YAML string.
     * @param yaml The YAML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    static validate(yaml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;
  }

  export class YAMLParser {
    /**
     * Parses a YAML string into a JavaScript object.
     * @param yaml The YAML string to parse.
     * @returns The parsed JavaScript object.
     */
    static parse(yaml: string): any;
  }

  export class YAMLBuilder {
    /**
     * Builds a YAML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @returns The serialized YAML string.
     */
    static build(obj: any): string;
  }

  export class TOMLValidator {
    /**
     * Validates a TOML string.
     * @param toml The TOML string to validate.
     * @param options Validation options.
     * @returns True if valid, false if invalid. If options.detailed is true, returns an object with valid and error properties.
     */
    static validate(toml: string, options?: ValidationOptions): boolean | ValidationDetailedResult;
  }

  export class TOMLParser {
    /**
     * Parses a TOML string into a JavaScript object.
     * @param toml The TOML string to parse.
     * @returns The parsed JavaScript object.
     */
    static parse(toml: string): any;
  }

  export class TOMLBuilder {
    /**
     * Builds a TOML string from a JavaScript object.
     * @param obj The JavaScript object to serialize. The root must be an object (TOML table).
     * @returns The serialized TOML string.
     */
    static build(obj: any): string;
  }



  export interface JSONLParseError {
    line: number;
    raw: string;
    cause: Error;
  }

  export interface JSONLDecoderOptions {
    skipInvalid?: boolean;
  }

  export class JSONLDecoderStream<T = any> extends TransformStream<string | Uint8Array, T> {
    constructor(options?: JSONLDecoderOptions);
    onError(callback: (error: JSONLParseError) => void): void;
  }

  export class JSONLEncoderStream<T = any> extends TransformStream<T, string> {
    constructor();
    pipeTo(destination: WritableStream<string | Uint8Array>, options?: StreamPipeOptions): Promise<void>;
    write(chunk: T): Promise<void>;
    close(): Promise<void>;
  }


}
