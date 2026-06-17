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
    if (result.startsWith("Parse failed:")) {
      throw new Error(result);
    }
    return JSON.parse(result);
  }
}

export class XMLBuilder {
  static build(obj, options = {}) {
    const result = xml_build(JSON.stringify(obj));
    if (result.startsWith("Parse failed:") || result.startsWith("Build failed:")) {
      throw new Error(result);
    }
    return result;
  }
}

export class XMLDecoderStream extends TransformStream {
  constructor(options = {}) {
    super({
      transform(chunk, controller) {
        // stream stub
        controller.enqueue(chunk);
      }
    });
  }
}
