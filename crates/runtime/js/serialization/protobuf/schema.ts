// Public Protobuf API. Compiles a .proto schema (a single source string, or a
// map of filename → source for multi-file schemas with imports) at runtime and
// decodes/encodes messages reflectively against it.
import { type ParsedFile, parseProto } from "./parser.js";
import { type Registry, link } from "./link.js";
import { decode } from "./decode.js";
import { encode } from "./encode.js";
import { Reader } from "./reader.js";
import { Writer } from "./writer.js";
import { WKT } from "./wkt.js";

export interface SchemaOptions {
  /** Entry filename when `proto` is a file map (defaults to all files). */
  entry?: string;
}

export class Schema {
  private registry: Registry;

  constructor(proto: string | Record<string, string>, _opts: SchemaOptions = {}) {
    const sources: Record<string, string> =
      typeof proto === "string" ? { "__main__.proto": proto } : { ...proto };

    const parsed: ParsedFile[] = [];
    const seen = new Set<string>();
    const toParse = Object.keys(sources);

    while (toParse.length) {
      const name = toParse.shift()!;
      if (seen.has(name)) continue;
      seen.add(name);
      const src = sources[name] ?? WKT[name];
      if (src == null) throw new Error(`protobuf: cannot resolve import "${name}"`);
      const pf = parseProto(src);
      parsed.push(pf);
      for (const imp of pf.imports) if (!seen.has(imp)) toParse.push(imp);
    }

    this.registry = link(parsed);
  }

  /** Decodes binary protobuf bytes for the fully-qualified `messageName`. */
  decode(messageName: string, bytes: Uint8Array): Record<string, unknown> {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    return decode(m, new Reader(bytes));
  }

  /** Encodes `value` as binary protobuf for the fully-qualified `messageName`. */
  encode(messageName: string, value: Record<string, unknown>): Uint8Array {
    const m = this.registry.messages.get(messageName);
    if (!m) throw new Error(`protobuf: unknown message "${messageName}"`);
    const w = new Writer();
    encode(m, value, w);
    return w.finish();
  }
}

export const Protobuf = { Schema };
