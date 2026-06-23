// runtime:serialization — XML/YAML/TOML/JSONL/MessagePack, backed by host ops.
const {
  xml_parse, xml_validate, xml_build,
  yaml_parse, yaml_validate, yaml_build,
  toml_parse, toml_validate, toml_build,
  msgpack_parse, msgpack_validate, msgpack_build,
  xml_stream_new, xml_stream_push, xml_stream_close
} = globalThis.__ops;

export const TOML = {
  validate(toml, options = {}) {
    const result = toml_validate(toml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  },
  parse(toml) {
    // toml_parse builds the value directly (datetimes as RFC3339 strings).
    return toml_parse(toml);
  },
  build(obj) {
    return toml_build(obj);
  }
};

export const YAML = {
  validate(yaml, options = {}) {
    const result = yaml_validate(yaml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  },
  parse(yaml) {
    // yaml_parse builds the value directly so .inf/.nan survive as Infinity/NaN.
    return yaml_parse(yaml);
  },
  build(obj) {
    return yaml_build(obj);
  }
};

export const MessagePack = {
  validate(msgpack, options = {}) {
    const result = msgpack_validate(msgpack);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  },
  decode(msgpack) {
    return JSON.parse(msgpack_parse(msgpack));
  },
  encode(obj) {
    return msgpack_build(obj);
  }
};

class JSONLDecoderStream extends TransformStream {
  constructor(options = {}) {
    let buffer = '';
    const decoder = new TextDecoder();
    const skipInvalid = !!options.skipInvalid;
    let lineNumber = 0;
    
    let errorCallback = null;

    super({
      transform(chunk, controller) {
        const text = typeof chunk === 'string' ? chunk : decoder.decode(chunk, { stream: true });
        buffer += text;
        const lines = buffer.split('\n');
        buffer = lines.pop(); // The last part might be incomplete
        
        for (const line of lines) {
          lineNumber++;
          const trimmed = line.trim();
          if (trimmed) {
            try {
              controller.enqueue(JSON.parse(trimmed));
            } catch (err) {
              if (skipInvalid) {
                if (errorCallback) {
                  errorCallback({ line: lineNumber, raw: trimmed, cause: err });
                }
              } else {
                controller.error(new SyntaxError(`Invalid JSONL line ${lineNumber}: ${err.message}`));
              }
            }
          }
        }
      },
      flush(controller) {
        if (buffer) {
          lineNumber++;
          const trimmed = buffer.trim();
          if (trimmed) {
            try {
              controller.enqueue(JSON.parse(trimmed));
            } catch (err) {
              if (skipInvalid) {
                if (errorCallback) {
                  errorCallback({ line: lineNumber, raw: trimmed, cause: err });
                }
              } else {
                controller.error(new SyntaxError(`Invalid JSONL line ${lineNumber}: ${err.message}`));
              }
            }
          }
        }
      }
    });
    
    this.onError = (cb) => {
      errorCallback = cb;
    };
  }
}

class JSONLEncoderStream extends TransformStream {
  constructor() {
    super({
      transform(chunk, controller) {
        try {
          controller.enqueue(JSON.stringify(chunk) + '\n');
        } catch (err) {
          controller.error(new TypeError(`Cannot serialize to JSONL: ${err.message}`));
        }
      }
    });
    this._writer = null;
  }

  pipeTo(destination, options) {
    return this.readable.pipeTo(destination, options);
  }

  write(chunk) {
    if (!this._writer) {
      this._writer = this.writable.getWriter();
    }
    return this._writer.write(chunk);
  }

  close() {
    if (this._writer) {
      return this._writer.close();
    }
    const writer = this.writable.getWriter();
    return writer.close();
  }
}

export const JSONL = {
  DecoderStream: JSONLDecoderStream,
  EncoderStream: JSONLEncoderStream
};

class XMLDecoderStream extends TransformStream {
  constructor(options = {}) {
    let streamId = null;

    super({
      start(controller) {
        streamId = xml_stream_new();
      },
      transform(chunk, controller) {
        const text = typeof chunk === 'string' ? chunk : new TextDecoder().decode(chunk);
        const parsedObjects = xml_stream_push(streamId, text);
        for (const obj of parsedObjects) {
          controller.enqueue(obj);
        }
      },
      flush(controller) {
        xml_stream_close(streamId);
      }
    });
  }
}

export const XML = {
  validate(xml, options = {}) {
    const result = xml_validate(xml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  },
  parse(xml) {
    return xml_parse(xml);
  },
  build(obj) {
    return xml_build(obj);
  },
  DecoderStream: XMLDecoderStream
};
