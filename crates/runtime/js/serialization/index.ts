// runtime:serialization — entry bundled into
// crates/runtime/src/runtime_modules/serialization.js (via `bun run build`).
//
// XML/YAML/TOML/JSONL/MessagePack are thin wrappers over the Rust host ops;
// Protobuf is a pure-JS reflective implementation (./protobuf).
export { Protobuf } from "./protobuf/schema.js";

const ops = (globalThis as unknown as { __ops: Record<string, (...a: any[]) => any> }).__ops;
const {
  xml_parse, xml_validate, xml_build,
  yaml_parse, yaml_validate, yaml_build,
  toml_parse, toml_validate, toml_build,
  msgpack_parse, msgpack_validate, msgpack_build,
  xml_stream_new, xml_stream_push, xml_stream_close,
} = ops;

interface ValidateOptions { detailed?: boolean; }
function validateWith(fn: (s: any) => true | string, input: any, options: ValidateOptions = {}) {
  const result = fn(input);
  if (result === true) return options.detailed ? { valid: true } : true;
  return options.detailed ? { valid: false, error: result } : false;
}

export const TOML = {
  validate: (toml: string, options?: ValidateOptions) => validateWith(toml_validate, toml, options),
  parse: (toml: string) => toml_parse(toml),
  build: (obj: unknown) => toml_build(obj),
};

export const YAML = {
  validate: (yaml: string, options?: ValidateOptions) => validateWith(yaml_validate, yaml, options),
  parse: (yaml: string) => yaml_parse(yaml),
  build: (obj: unknown) => yaml_build(obj),
};

export const MessagePack = {
  validate: (msgpack: Uint8Array, options?: ValidateOptions) => validateWith(msgpack_validate, msgpack, options),
  decode: (msgpack: Uint8Array) => JSON.parse(msgpack_parse(msgpack)),
  encode: (obj: unknown) => msgpack_build(obj),
};

class JSONLDecoderStream extends TransformStream {
  onError: (cb: (e: { line: number; raw: string; cause: Error }) => void) => void;
  constructor(options: { skipInvalid?: boolean } = {}) {
    let buffer = "";
    const decoder = new TextDecoder();
    const skipInvalid = !!options.skipInvalid;
    let lineNumber = 0;
    let errorCallback: ((e: { line: number; raw: string; cause: Error }) => void) | null = null;

    const emit = (controller: TransformStreamDefaultController, raw: string) => {
      const trimmed = raw.trim();
      if (!trimmed) return;
      try {
        controller.enqueue(JSON.parse(trimmed));
      } catch (err) {
        if (skipInvalid) errorCallback?.({ line: lineNumber, raw: trimmed, cause: err as Error });
        else controller.error(new SyntaxError(`Invalid JSONL line ${lineNumber}: ${(err as Error).message}`));
      }
    };

    super({
      transform(chunk, controller) {
        const text = typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
        buffer += text;
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) { lineNumber++; emit(controller, line); }
      },
      flush(controller) {
        if (buffer) { lineNumber++; emit(controller, buffer); }
      },
    });

    this.onError = (cb) => { errorCallback = cb; };
  }
}

class JSONLEncoderStream extends TransformStream {
  private _writer: WritableStreamDefaultWriter | null = null;
  constructor() {
    super({
      transform(chunk, controller) {
        try {
          controller.enqueue(JSON.stringify(chunk) + "\n");
        } catch (err) {
          controller.error(new TypeError(`Cannot serialize to JSONL: ${(err as Error).message}`));
        }
      },
    });
  }
  pipeTo(destination: WritableStream, options?: StreamPipeOptions) {
    return this.readable.pipeTo(destination, options);
  }
  write(chunk: unknown) {
    this._writer ??= this.writable.getWriter();
    return this._writer.write(chunk);
  }
  close() {
    return (this._writer ?? this.writable.getWriter()).close();
  }
}

export const JSONL = { DecoderStream: JSONLDecoderStream, EncoderStream: JSONLEncoderStream };

class XMLDecoderStream extends TransformStream {
  constructor() {
    let streamId: number | null = null;
    super({
      start() { streamId = xml_stream_new(); },
      transform(chunk, controller) {
        const text = typeof chunk === "string" ? chunk : new TextDecoder().decode(chunk);
        for (const obj of xml_stream_push(streamId, text)) controller.enqueue(obj);
      },
      flush() { xml_stream_close(streamId); },
    });
  }
}

export const XML = {
  validate: (xml: string, options?: ValidateOptions) => validateWith(xml_validate, xml, options),
  parse: (xml: string) => xml_parse(xml),
  build: (obj: unknown) => xml_build(obj),
  DecoderStream: XMLDecoderStream,
};
