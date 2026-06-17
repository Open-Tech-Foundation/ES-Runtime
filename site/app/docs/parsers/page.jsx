import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

export default function ParsersDoc() {
  return (
    <DocsShell active="/docs/parsers">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Native parsers (XML)
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime includes highly optimized native parsers accessible via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module. These operations run directly in Rust, completely avoiding JavaScript garbage collection overhead and offering best-in-class performance.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        XML Processing
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module exposes <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLParser</code>, <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLBuilder</code>, and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">XMLValidator</code>. These classes use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">quick-xml</code> under the hood to perform ultra-fast XML-to-JSON and JSON-to-XML conversion.
      </p>

      <div className="mt-6">
        <CodeBlock code={`import { XMLParser, XMLValidator, XMLBuilder } from "runtime:parsers";

const xmlData = \`<user id="1"><name>Alice</name></user>\`;

// 1. Validation
if (XMLValidator.validate(xmlData)) {
  console.log("XML is valid!");
}

// 2. Parsing (XML to JS Object)
const parsed = XMLParser.parse(xmlData);
console.log(parsed.name.$text); // "Alice"

// 3. Building (JS Object to XML)
const built = XMLBuilder.build({ user: { "@id": "1", name: "Alice" } });
console.log(built); // <user id="1"><name>Alice</name></user>`} title="parsers.js" lang="js" />
      </div>

      <h3 className="mt-8 text-xl font-semibold text-zinc-900">Performance</h3>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Because parsing happens directly within the Rust native core, <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> operates around 10% faster than <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">fast-xml-parser</code> running on Node.js or Bun, while utilizing half the memory. You can view the full benchmarks on the <a href="/docs/benchmarks" className="text-brand-600 hover:underline">Benchmarks</a> page.
      </p>
    </DocsShell>
  );
}
