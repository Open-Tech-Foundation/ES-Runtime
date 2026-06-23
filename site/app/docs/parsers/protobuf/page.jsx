import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function ProtobufParserDoc() {
  return (
    <DocsShell active="/docs/parsers/protobuf">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Protobuf Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        Native Protobuf parsing via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module, backed by <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">protox</code> and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">prost-reflect</code>. It compiles <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.proto</code> schemas in memory and translates between JavaScript objects and binary Protobuf using the proto3 JSON mapping.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Dynamic Schema Compilation
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        You can dynamically compile <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.proto</code> files into active schema objects using the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Protobuf.Schema</code> class.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { Protobuf } from "runtime:parsers";

const protoDefinition = \`
  syntax = "proto3";
  package api;

  message User {
    int32 id = 1;
    string name = 2;
  }
\`;

const schema = new Protobuf.Schema(protoDefinition);
console.log(schema);`} title="protobuf_schema.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Parsing Protobuf
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.parse</code> to convert a Protobuf byte array into a JavaScript object, given the fully-qualified message name.
      </p>
      <div className="mt-6">
        <CodeBlock code={`// Continuing from above...
const pbBytes = new Uint8Array([8, 1, 18, 5, 65, 108, 105, 99, 101]); // Encoded { id: 1, name: "Alice" }

const user = schema.parse("api.User", pbBytes);
console.log(user.name); // "Alice"
console.log(user.id);   // 1`} title="protobuf_parse.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Building Protobuf
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.build</code> to convert a JavaScript object into a binary Protobuf <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Uint8Array</code>.
      </p>
      <div className="mt-6">
        <CodeBlock code={`// Continuing from above...
const obj = { 
  id: 2, 
  name: "Bob" 
};

const built = schema.build("api.User", obj);
console.log(built instanceof Uint8Array); // true`} title="protobuf_build.js" lang="js" />
      </div>
    </DocsShell>
  );
}
