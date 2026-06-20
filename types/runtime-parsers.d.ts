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
     * @param options Parsing options.
     * @returns The parsed JavaScript object.
     */
    static parse(xml: string, options?: object): any;
  }

  export class XMLBuilder {
    /**
     * Builds an XML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @param options Building options.
     * @returns The serialized XML string.
     */
    static build(obj: any, options?: object): string;
  }

  export class XMLDecoderStream extends TransformStream<string | Uint8Array, any> {
    constructor(options?: object);
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
     * @param options Parsing options.
     * @returns The parsed JavaScript object.
     */
    static parse(yaml: string, options?: object): any;
  }

  export class YAMLBuilder {
    /**
     * Builds a YAML string from a JavaScript object.
     * @param obj The JavaScript object to serialize.
     * @param options Building options.
     * @returns The serialized YAML string.
     */
    static build(obj: any, options?: object): string;
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
     * @param options Parsing options.
     * @returns The parsed JavaScript object.
     */
    static parse(toml: string, options?: object): any;
  }

  export class TOMLBuilder {
    /**
     * Builds a TOML string from a JavaScript object.
     * @param obj The JavaScript object to serialize. The root must be an object (TOML table).
     * @param options Building options.
     * @returns The serialized TOML string.
     */
    static build(obj: any, options?: object): string;
  }
}
