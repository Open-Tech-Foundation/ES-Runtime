const { 
  xml_parse, xml_validate, xml_build,
  yaml_parse, yaml_validate, yaml_build,
  toml_parse, toml_validate, toml_build
} = globalThis.__ops;

export class TOMLValidator {
  static validate(toml, options = {}) {
    const result = toml_validate(toml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  }
}

export class TOMLParser {
  static parse(toml) {
    return toml_parse(toml);
  }
}

export class TOMLBuilder {
  static build(obj) {
    return toml_build(obj);
  }
}

export class YAMLValidator {
  static validate(yaml, options = {}) {
    const result = yaml_validate(yaml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  }
}

export class YAMLParser {
  static parse(yaml) {
    return yaml_parse(yaml);
  }
}

export class YAMLBuilder {
  static build(obj) {
    return yaml_build(obj);
  }
}



export class JSONLDecoderStream extends TransformStream {
  constructor(options = {}) {
    let buffer = '';
    const decoder = new TextDecoder();
    const skipInvalid = !!options.skipInvalid;
    let lineNumber = 0;
    
    // We need a reference to the instance to store the callback,
    // but we are inside the constructor. We can define a local variable
    // for the callback and a method on 'this' to set it.
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

export class JSONLEncoderStream extends TransformStream {
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



export class XMLValidator {
  static validate(xml, options = {}) {
    const result = xml_validate(xml);
    if (result === true) {
      if (options.detailed) return { valid: true };
      return true;
    }
    if (options.detailed) return { valid: false, error: result };
    return false;
  }
}

export class XMLParser {
  static parse(xml) {
    // xml_parse throws (SyntaxError) on malformed input, so a string result
    // here is genuine parsed text content, never an error sentinel.
    return xml_parse(xml);
  }
}

export class XMLBuilder {
  static build(obj) {
    // xml_build throws (TypeError) if the value can't be serialized.
    return xml_build(obj);
  }
}

export class XMLDecoderStream extends TransformStream {
  constructor(options = {}) {
    let streamId = null;

    super({
      start(controller) {
        streamId = globalThis.__ops.xml_stream_new();
      },
      transform(chunk, controller) {
        const text = typeof chunk === 'string' ? chunk : new TextDecoder().decode(chunk);
        const parsedObjects = globalThis.__ops.xml_stream_push(streamId, text);
        for (const obj of parsedObjects) {
          controller.enqueue(obj);
        }
      },
      flush(controller) {
        globalThis.__ops.xml_stream_close(streamId);
      }
    });
  }
}
