const { xml_parse, xml_validate, xml_build } = globalThis.__ops;

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
    const result = xml_parse(xml);
    if (typeof result === 'string' && result.startsWith("Parse failed:")) {
      throw new Error(result);
    }
    return result;
  }
}

export class XMLBuilder {
  static build(obj, options = {}) {
    const result = xml_build(obj);
    if (typeof result === 'string' && (result.startsWith("Parse failed:") || result.startsWith("Build failed:"))) {
      throw new Error(result);
    }
    return result;
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
