import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function XMLParserDoc() {
  return (
    <DocsShell active="/docs/parsers/xml">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        XML Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime includes a highly optimized native XML parser accessible via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module. These operations run directly in Rust, completely avoiding JavaScript garbage collection overhead and offering best-in-class performance.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Parsing XML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLParser.parse</code> to convert an XML string directly into a JavaScript object.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { XMLParser } from "runtime:parsers";

const xmlData = \`<user id="1"><name>Alice</name></user>\`;

const parsed = XMLParser.parse(xmlData);
console.log(parsed.name.$text); // "Alice"
console.log(parsed["@id"]);     // "1"`} title="xml_parse.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Validating XML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLValidator.validate</code> to check if an XML string is well-formed. Pass <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">{'{'} detailed: true {'}'}</code> to get specific error messages.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { XMLValidator } from "runtime:parsers";

const xmlData = \`<user id="1"><name>Alice</name></user>\`;

if (XMLValidator.validate(xmlData)) {
  console.log("XML is valid!");
}

const result = XMLValidator.validate("<broken>", { detailed: true });
console.log(result.valid); // false
console.log(result.error); // "Validation failed: ..." `} title="xml_validate.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Building XML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLBuilder.build</code> to convert a JavaScript object back into an XML string.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { XMLBuilder } from "runtime:parsers";

const obj = { 
  user: { 
    "@id": "1", 
    name: "Alice" 
  } 
};

const built = XMLBuilder.build(obj);
console.log(built); // <user id="1"><name>Alice</name></user>`} title="xml_build.js" lang="js" />
      </div>

      <h3 className="mt-8 text-xl font-semibold text-zinc-900">Streaming (XMLDecoderStream)</h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        For massive multi-gigabyte XML datasets, ES-Runtime provides the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLDecoderStream</code>. This is a standard Web <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">TransformStream</code> that consumes XML string chunks and incrementally yields fully-parsed JavaScript objects, achieving a near-zero memory footprint.
      </p>

      <div className="mt-6">
        <CodeBlock code={`import { XMLDecoderStream } from "runtime:parsers";

async function processMassiveFeed(fileStream) {
  // Pipe the chunks directly into the native streaming parser
  const objectStream = fileStream.pipeThrough(new XMLDecoderStream());

  // Use async iteration to consume parsed top-level element objects natively
  for await (const value of objectStream) {
    console.log(value);
  }
}`} title="streaming.js" lang="js" />
      </div>

      <h3 className="mt-8 text-xl font-semibold text-zinc-900">Performance</h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Because parsing happens directly within the Rust native core, <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> operates around 10% faster than <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">fast-xml-parser</code> running on Node.js or Bun, while utilizing half the memory. You can view the full benchmarks on the <a href="/docs/benchmarks" className="text-brand-600 hover:underline">Benchmarks</a> page.
      </p>
    </DocsShell>
  );
}
