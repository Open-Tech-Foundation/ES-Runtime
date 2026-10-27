// runtime:serialization — entry bundled into
// crates/runtime/src/runtime_modules/serialization.js (via `pnpm run build`).
//
// XML/YAML/TOML/CSV/JSONL/MessagePack/CBOR are thin wrappers over the Rust host
// ops; BSON encodes and decodes in engine builtins over live values (./bson.ts);
// Protobuf is a pure-JS reflective implementation (./protobuf).

export { BSON } from "./bson.ts";
export { Protobuf } from "./protobuf/schema.ts";

import { BSON as BSONDefault } from "./bson.ts";
import { Protobuf as ProtobufDefault } from "./protobuf/schema.ts";

const ops = (globalThis as unknown as { __ops: Record<string, (...a: any[]) => any> }).__ops;
const {
  xml_parse,
  xml_validate,
  xml_build,
  yaml_parse,
  yaml_validate,
  yaml_build,
  toml_parse,
  toml_validate,
  toml_build,
  csv_parse,
  csv_validate,
  csv_build,
  csv_stream_new,
  csv_stream_push,
  csv_stream_close,
  csv_encode_new,
  csv_encode_push,
  csv_encode_close,
  msgpack_parse,
  msgpack_validate,
  msgpack_build,
  cbor_parse,
  cbor_validate,
  cbor_build,
  xml_stream_new,
  xml_stream_push,
  xml_stream_close,
} = ops;

interface ValidateOptions {
  detailed?: boolean;
}
function validateWith(fn: (s: any) => true | string, input: any, options: ValidateOptions = {}) {
  const result = fn(input);
  if (result === true) return options.detailed ? { valid: true } : true;
  return options.detailed ? { valid: false, error: result } : false;
}

export const TOML = {
  validate: (toml: string, options?: ValidateOptions) => validateWith(toml_validate, toml, options),
  parse: (toml: string) => toml_parse(toml),
  build: (obj: unknown) => toml_build(toEncodable(obj, "TOML")),
};

export interface CSVOptions {
  delimiter?: string;
  quote?: string;
  escape?: string;
  header?: boolean | string[];
  trim?: boolean;
  skipEmptyLines?: boolean;
  comment?: string;
  cast?: boolean;
  relaxColumnCount?: boolean;
  maxFieldBytes?: number;
  maxColumns?: number;
  bom?: boolean;
  newline?: "\n" | "\r\n" | "\r";
}

export interface CSVValidateOptions extends CSVOptions {
  detailed?: boolean;
}

// Streaming decode. Unlike JSONL, which can split a chunk on `\n` in JS, a CSV
// field may contain quoted newlines and quotes, and a chunk may break between a
// `\r` and its `\n`, inside a doubled quote, or across the escape character — so
// the scanner cannot live here. The Rust op holds that state (csv.rs
// `StreamDecoder`); this only decodes bytes with a streaming TextDecoder and
// forwards each completed row. `flush` returns the last record and reports an
// unterminated quote, both of which are only knowable at the end.
class CSVDecoderStream extends TransformStream<string | Uint8Array, unknown> {
  constructor(options: CSVOptions = {}) {
    // One decoder for the whole stream: `{ stream: true }` only buffers a
    // partial multi-byte sequence if the *same* decoder sees the next chunk.
    // `ignoreBOM` passes a leading BOM through so the host's `bom` option
    // decides, as it does for `parse`; by default TextDecoder would strip it
    // and `bom: false` would mean nothing for byte input.
    const decoder = new TextDecoder("utf-8", { ignoreBOM: true });
    let streamId: number | null = null;
    super({
      start() {
        streamId = csv_stream_new(options);
      },
      transform(chunk, controller) {
        const text = typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
        for (const row of csv_stream_push(streamId, text)) controller.enqueue(row);
      },
      flush(controller) {
        // A trailing incomplete UTF-8 sequence decodes to U+FFFD; anything the
        // decoder still held is the last field's bytes, so push it before close.
        const tail = decoder.decode();
        if (tail) for (const row of csv_stream_push(streamId, tail)) controller.enqueue(row);
        for (const row of csv_stream_close(streamId)) controller.enqueue(row);
      },
      // A canceled readable or aborted writable never reaches `flush`, so the
      // host state (and any partial field it holds) is released here. Its rows
      // and errors are unwanted: the consumer has already walked away.
      cancel() {
        try {
          csv_stream_close(streamId);
        } catch {
          // an unterminated quote at cancel is not an error anyone can see
        }
      },
    });
  }
}

// Streaming encode: one row per chunk, matching `CSV.build`'s row semantics. The
// first row fixes the shape and, for objects, the header, so the Rust op keeps
// that state and this reuses the same writer as `build`.
class CSVEncoderStream extends TransformStream<unknown, string> {
  private _writer: WritableStreamDefaultWriter | null = null;
  constructor(options: CSVOptions = {}) {
    let streamId: number | null = null;
    super({
      start() {
        streamId = csv_encode_new(options);
      },
      transform(chunk, controller) {
        const text = csv_encode_push(streamId, chunk);
        if (text) controller.enqueue(text);
      },
      flush() {
        csv_encode_close(streamId);
      },
      cancel() {
        csv_encode_close(streamId);
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

// CSV is the one format whose *validity* depends on the parse options: a ragged
// row is fine by default but an error once `header` is set, so `validate` has to
// forward the options rather than just the text the way XML/YAML/TOML do. The
// `detailed` flag is ignored by the host, which only reads the parse options.
export const CSV = {
  validate: (csv: string, options: CSVValidateOptions = {}) => {
    const result = csv_validate(csv, options);
    if (result === true) return options.detailed ? { valid: true } : true;
    return options.detailed ? { valid: false, error: result } : false;
  },
  parse: (csv: string, options?: CSVOptions) => csv_parse(csv, options),
  build: (rows: unknown, options?: CSVOptions) => csv_build(rows, options),
  DecoderStream: CSVDecoderStream,
  EncoderStream: CSVEncoderStream,
};

export const YAML = {
  validate: (yaml: string, options?: ValidateOptions) => validateWith(yaml_validate, yaml, options),
  parse: (yaml: string) => yaml_parse(yaml),
  build: (obj: unknown) => yaml_build(toEncodable(obj, "YAML")),
};

// Values that cross to the host as an *empty object* because they carry no own
// enumerable properties — a `Map`, a `Set`, a `Date`, an `ArrayBuffer`. Encoding
// those as `{}` is silent data loss, so every builder (the text formats too: a
// `Date` used to become an empty TOML table) converts the ones it can carry to a
// shape that survives, and the encoder refuses the rest — the text formats
// refuse bytes.
//
// Only containers are walked, and only on the encode path.
//
// `seen` is the path from the root, not every value walked, so a container
// reachable twice by different routes is still converted twice — only a genuine
// cycle is refused. Without the check a cycle bottomed out on the depth cap
// instead, reporting "nests too deeply" for a document that is not deep at all.
//
// `format` names the caller in the messages, since every builder shares this
// conversion.
function toEncodable(value: unknown, format: string, depth = 0, seen: object[] = []): unknown {
  if (depth > 256) throw new RangeError(`${format}: value nests too deeply to encode`);
  if (value === null || typeof value !== "object") return value;
  // A boxed primitive keeps its value in an internal slot, so it would cross as
  // `{}` (`new Number(1)`) or as its indexed characters (`new String("x")` →
  // `{ "0": "x" }`). It is the primitive it wraps, as structuredClone treats it.
  if (
    value instanceof String ||
    value instanceof Number ||
    value instanceof Boolean ||
    value instanceof BigInt
  )
    return value.valueOf();
  if (value instanceof Uint8Array) return value;
  // Any other view, and a bare buffer, become bytes — this is a binary format.
  if (ArrayBuffer.isView(value)) {
    const v = value as ArrayBufferView;
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  }
  if (value instanceof ArrayBuffer) return new Uint8Array(value);
  if (value instanceof Date) return value.toISOString();
  if (seen.includes(value)) throw new TypeError("Converting circular structure to a host value");
  seen = [...seen, value];
  // A null-prototype object is what carries the keys across: assigning to an
  // ordinary `{}` runs any setter the key names on `Object.prototype` — a
  // `__proto__` key would set the prototype and silently drop the field — so
  // there is no prototype to intercept the assignment.
  if (value instanceof Map) {
    const out: Record<string, unknown> = Object.create(null);
    for (const [k, v] of value) out[String(k)] = toEncodable(v, format, depth + 1, seen);
    return out;
  }
  if (value instanceof Set) return [...value].map((v) => toEncodable(v, format, depth + 1, seen));
  if (Array.isArray(value)) return value.map((v) => toEncodable(v, format, depth + 1, seen));
  // Anything else whose brand is not a plain object — a RegExp, an Error, a
  // URL, a WeakMap, a Promise — keeps its state in internal slots and would be
  // written as `{}`. js-yaml refuses these the same way (by the same
  // `Object.prototype.toString` test); cbor-x keeps some through tags, which
  // these codecs do not write. A class instance brands as `[object Object]` and
  // is written as a map of its own enumerable properties, as both do.
  const brand = Object.prototype.toString.call(value);
  if (brand !== "[object Object]")
    throw new TypeError(`${format}: cannot encode ${brand.slice(8, -1)}; it has no ${format} form`);
  const out: Record<string, unknown> = Object.create(null);
  for (const k of Object.keys(value))
    out[k] = toEncodable((value as Record<string, unknown>)[k], format, depth + 1, seen);
  return out;
}

export const MessagePack = {
  validate: (msgpack: Uint8Array, options?: ValidateOptions) =>
    validateWith(msgpack_validate, msgpack, options),
  // The host returns a JSON *string* for a JSON-shaped document — the fast
  // pivot, parsed here — and the decoded value itself when the document
  // carries `bin`/`ext`, which JSON cannot represent. A string result is
  // therefore always JSON to parse, never a decoded top-level string: that
  // case arrives pre-decoded.
  decode: (msgpack: Uint8Array) => {
    const decoded = msgpack_parse(msgpack);
    return typeof decoded === "string" ? JSON.parse(decoded) : decoded;
  },
  encode: (obj: unknown) => msgpack_build(toEncodable(obj, "MessagePack")),
};

// CBOR decodes straight to a value — never through the JSON string pivot — so
// there is no string/value ambiguity to resolve here, and a top-level string
// arrives as a string rather than as a document to parse.
export const CBOR = {
  validate: (cbor: Uint8Array, options?: ValidateOptions) =>
    validateWith(cbor_validate, cbor, options),
  decode: (cbor: Uint8Array) => cbor_parse(cbor),
  encode: (obj: unknown) => cbor_build(toEncodable(obj, "CBOR")),
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
        else
          controller.error(
            new SyntaxError(`Invalid JSONL line ${lineNumber}: ${(err as Error).message}`),
          );
      }
    };

    super({
      transform(chunk, controller) {
        const text = typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
        buffer += text;
        const lines = buffer.split("\n");
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          lineNumber++;
          emit(controller, line);
        }
      },
      flush(controller) {
        if (buffer) {
          lineNumber++;
          emit(controller, buffer);
        }
      },
    });

    this.onError = (cb) => {
      errorCallback = cb;
    };
  }
}

class JSONLEncoderStream extends TransformStream {
  private _writer: WritableStreamDefaultWriter | null = null;
  constructor() {
    super({
      transform(chunk, controller) {
        try {
          controller.enqueue(`${JSON.stringify(chunk)}\n`);
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

// One record per direct child of the root. The scan and the record parser live
// in the host (xml.rs `StreamDecoder`), the same reader `parse` uses; `close`
// checks the end of the document, so a truncated one errors the stream.
class XMLDecoderStream extends TransformStream {
  constructor() {
    // One decoder for the whole stream: a new one per chunk turned a character
    // split across chunks into two U+FFFDs.
    const decoder = new TextDecoder();
    let streamId: number | null = null;
    super({
      start() {
        streamId = xml_stream_new();
      },
      transform(chunk, controller) {
        const text = typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
        for (const obj of xml_stream_push(streamId, text)) controller.enqueue(obj);
      },
      flush(controller) {
        const tail = decoder.decode();
        if (tail) for (const obj of xml_stream_push(streamId, tail)) controller.enqueue(obj);
        xml_stream_close(streamId);
      },
      // A canceled readable or aborted writable never reaches `flush`; release
      // the host state here, ignoring the truncation nobody is reading.
      cancel() {
        try {
          xml_stream_close(streamId);
        } catch {
          // the stream ended early on purpose
        }
      },
    });
  }
}

export const XML = {
  validate: (xml: string, options?: ValidateOptions) => validateWith(xml_validate, xml, options),
  parse: (xml: string) => xml_parse(xml),
  build: (obj: unknown) => xml_build(toEncodable(obj, "XML")),
  DecoderStream: XMLDecoderStream,
};

// The aggregate default, so `import serialization from "runtime:serialization"`
// works — every other `runtime:` module offers one, and this was the only
// exception, which made the namespace inconsistent for no reason a caller could
// see.
export default {
  XML,
  YAML,
  TOML,
  CSV,
  JSONL,
  MessagePack,
  CBOR,
  BSON: BSONDefault,
  Protobuf: ProtobufDefault,
};
