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

// Values that cross to the host as an *empty object* because they carry no own
// enumerable properties — a `Map`, a `Set`, an `ArrayBuffer`. Encoding those as
// `{}` is silent data loss, so the ones MessagePack can carry faithfully are
// converted to a shape that survives, and the rest are refused by the encoder.
//
// Only containers are walked, and only on the encode path.
function toEncodable(value: unknown, depth = 0): unknown {
  if (depth > 256) throw new RangeError("MessagePack: value nests too deeply to encode");
  if (value === null || typeof value !== "object") return value;
  if (value instanceof Uint8Array) return value;
  // Any other view, and a bare buffer, become bytes — this is a binary format.
  if (ArrayBuffer.isView(value)) {
    const v = value as ArrayBufferView;
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  }
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (value instanceof Map) {
    const out: Record<string, unknown> = {};
    for (const [k, v] of value) out[String(k)] = toEncodable(v, depth + 1);
    return out;
  }
  if (value instanceof Set) return [...value].map((v) => toEncodable(v, depth + 1));
  if (Array.isArray(value)) return value.map((v) => toEncodable(v, depth + 1));
  if (value instanceof Date) return value.toISOString();
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(value)) out[k] = toEncodable((value as Record<string, unknown>)[k], depth + 1);
  return out;
}

export const MessagePack = {
  validate: (msgpack: Uint8Array, options?: ValidateOptions) => validateWith(msgpack_validate, msgpack, options),
  // The host returns a JSON *string* for a JSON-shaped document — the fast
  // pivot, parsed here — and the decoded value itself when the document
  // carries `bin`/`ext`, which JSON cannot represent. A string result is
  // therefore always JSON to parse, never a decoded top-level string: that
  // case arrives pre-decoded.
  decode: (msgpack: Uint8Array) => {
    const decoded = msgpack_parse(msgpack);
    return typeof decoded === "string" ? JSON.parse(decoded) : decoded;
  },
  encode: (obj: unknown) => msgpack_build(toEncodable(obj)),
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
