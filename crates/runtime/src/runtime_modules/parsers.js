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
  static parse(toml, options = {}) {
    return toml_parse(toml);
  }
}

export class TOMLBuilder {
  static build(obj, options = {}) {
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
  static parse(yaml, options = {}) {
    return yaml_parse(yaml);
  }
}

export class YAMLBuilder {
  static build(obj, options = {}) {
    return yaml_build(obj);
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
  static parse(xml, options = {}) {
    // xml_parse throws (SyntaxError) on malformed input, so a string result
    // here is genuine parsed text content, never an error sentinel.
    return xml_parse(xml);
  }
}

export class XMLBuilder {
  static build(obj, options = {}) {
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
