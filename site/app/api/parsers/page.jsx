import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import ErrorTable from "../../../components/ErrorTable.jsx";

const errors = [
  { e: "SyntaxError", w: "Parsing fails due to malformed XML, YAML, or TOML input." },
  { e: "TypeError", w: "Building fails because the provided JavaScript object cannot be serialized into the target format (e.g., circular references, invalid keys)." },
  { e: "RangeError", w: "The input exceeds parser depth or memory limits (e.g., XML streaming buffer cap)." },
];

const IMPORT = `import { 
  JSONLDecoderStream, JSONLEncoderStream,
  XMLParser, XMLBuilder, XMLValidator, XMLDecoderStream,
  YAMLParser, YAMLBuilder, YAMLValidator,
  TOMLParser, TOMLBuilder, TOMLValidator 
} from "runtime:parsers";`;

const sections = [
  {
    title: "JSONL (JSON Lines)",
    desc: "Stream-based parsing and serializing of JSON Lines data. JSONL is heavily optimized for large file handling natively over streams.",
    exports: [
      { 
        sig: "new JSONLDecoderStream(options?)", 
        type: "TransformStream", 
        desc: "Parses streaming JSONL byte chunks into JS objects incrementally.", 
        options: [
          { name: "skipInvalid", type: "boolean", optional: true, default: "false", desc: "If true, skips invalid JSON lines instead of destroying the stream. Skipped lines are emitted to decoder.onError(err)." }
        ],
        ex: `stream.pipeThrough(new JSONLDecoderStream({ skipInvalid: true }))` 
      },
      { 
        sig: "new JSONLEncoderStream()", 
        type: "TransformStream", 
        desc: "Serializes a stream of JS objects into JSONL byte chunks. No options are currently supported.", 
        ex: `stream.pipeThrough(new JSONLEncoderStream())` 
      },
    ]
  },
  {
    title: "XML",
    desc: "Synchronous parsing, building, and validation of XML data, plus streaming support for massive documents.",
    exports: [
      { sig: "XMLParser.parse(xml)", type: "(string) => object", desc: "Parses an XML string into a JavaScript object.", ex: `XMLParser.parse("<root>hi</root>");` },
      { sig: "XMLBuilder.build(obj)", type: "(object) => string", desc: "Serializes a JavaScript object into an XML string.", ex: `XMLBuilder.build({ root: "hi" });` },
      { 
        sig: "XMLValidator.validate(xml, options?)", 
        type: "(string, object) => boolean | object", 
        desc: "Validates an XML string.", 
        options: [
          { name: "detailed", type: "boolean", optional: true, default: "false", desc: "If true, returns an object { valid: boolean, error?: string } instead of a boolean, providing the exact syntax error if validation fails." }
        ],
        ex: `XMLValidator.validate("<root>", { detailed: true }); // { valid: false, error: "..." }` 
      },
      { sig: "new XMLDecoderStream()", type: "TransformStream", desc: "A TransformStream that parses streaming XML byte chunks into JavaScript objects incrementally.", ex: `stream.pipeThrough(new XMLDecoderStream())` },
    ]
  },
  {
    title: "YAML",
    desc: "Synchronous parsing, building, and validation of YAML data.",
    exports: [
      { sig: "YAMLParser.parse(yaml)", type: "(string) => object", desc: "Parses a YAML string into a JavaScript object.", ex: `YAMLParser.parse("key: value");` },
      { sig: "YAMLBuilder.build(obj)", type: "(object) => string", desc: "Serializes a JavaScript object into a YAML string.", ex: `YAMLBuilder.build({ key: "value" });` },
      { 
        sig: "YAMLValidator.validate(yaml, options?)", 
        type: "(string, object) => boolean | object", 
        desc: "Validates a YAML string.", 
        options: [
          { name: "detailed", type: "boolean", optional: true, default: "false", desc: "If true, returns an object { valid: boolean, error?: string } instead of a boolean, providing the exact syntax error if validation fails." }
        ],
        ex: `YAMLValidator.validate("key: value", { detailed: true });` 
      },
    ]
  },
  {
    title: "TOML",
    desc: "Synchronous parsing, building, and validation of TOML data.",
    exports: [
      { sig: "TOMLParser.parse(toml)", type: "(string) => object", desc: "Parses a TOML string into a JavaScript object.", ex: `TOMLParser.parse("key = 'value'");` },
      { sig: "TOMLBuilder.build(obj)", type: "(object) => string", desc: "Serializes a JavaScript object into a TOML string. The root must be an object/table.", ex: `TOMLBuilder.build({ key: "value" });` },
      { 
        sig: "TOMLValidator.validate(toml, options?)", 
        type: "(string, object) => boolean | object", 
        desc: "Validates a TOML string.", 
        options: [
          { name: "detailed", type: "boolean", optional: true, default: "false", desc: "If true, returns an object { valid: boolean, error?: string } instead of a boolean, providing the exact syntax error if validation fails." }
        ],
        ex: `TOMLValidator.validate("key = 'value'", { detailed: true });` 
      },
    ]
  }
];

export default function ParsersApiDoc() {
  return (
    <ApiShell active="/api/parsers">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:parsers
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Native parser classes for JSONL, XML, YAML, and TOML. These operations run directly in Rust, avoiding JavaScript overhead and providing best-in-class performance.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: None (Pure Computation)
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · runtime: scheme
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Import</h2>
      <div className="mt-4">
        <CodeBlock code={IMPORT} title="runtime:parsers" lang="js" />
      </div>

      {sections.map(section => (
        <div key={section.title}>
          <h2 className="mt-12 text-xl font-semibold text-zinc-900">{section.title}</h2>
          <p className="mt-2 text-sm leading-relaxed text-zinc-600">{section.desc}</p>
          <div className="mt-5 space-y-4">
            {section.exports.map((e) => (
              <div className="rounded-xl border border-zinc-200 p-5" key={e.sig}>
                <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                  <code className="font-mono text-[15px] font-semibold text-zinc-900">
                    {e.sig}
                  </code>
                  <code className="font-mono text-[13px] text-zinc-400">
                    {e.type}
                  </code>
                </div>
                <p className="mt-2 text-sm leading-relaxed text-zinc-600">
                  {e.desc}
                </p>

                {e.options && (
                  <div className="mt-4 overflow-hidden rounded-lg border border-zinc-200">
                    <table className="w-full text-left text-sm">
                      <thead className="bg-zinc-50 text-zinc-500">
                        <tr>
                          <th className="px-4 py-2 font-medium">Name</th>
                          <th className="px-4 py-2 font-medium">Type</th>
                          <th className="px-4 py-2 font-medium">Default</th>
                          <th className="px-4 py-2 font-medium">Description</th>
                        </tr>
                      </thead>
                      <tbody className="divide-y divide-zinc-200 bg-white text-zinc-600">
                        {e.options.map((opt) => (
                          <tr key={opt.name}>
                            <td className="px-4 py-3 font-mono text-xs text-zinc-900">{opt.name}{opt.optional && '?'}</td>
                            <td className="px-4 py-3 font-mono text-xs text-brand-600">{opt.type}</td>
                            <td className="px-4 py-3 font-mono text-xs">{opt.default}</td>
                            <td className="px-4 py-3 leading-relaxed">{opt.desc}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                )}

                <code className="mt-4 block overflow-x-auto rounded-lg bg-zinc-950 px-3 py-2 font-mono text-[12px] text-emerald-300">
                  {e.ex}
                </code>
              </div>
            ))}
          </div>
        </div>
      ))}

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Errors</h2>
      <ErrorTable rows={errors} />
    </ApiShell>
  );
}
