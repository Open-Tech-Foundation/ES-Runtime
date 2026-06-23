import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function ProtobufParserDoc() {
  return (
    <DocsShell active="/docs/serialization/protobuf">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Protobuf Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        A pure-JavaScript, reflective Protobuf implementation in <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:serialization</code>. The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.proto</code> schema is compiled at runtime — no codegen, no build step. proto3 and editions 2023/2024 are supported; proto2-only constructs are rejected.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Compiling a schema
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Construct a <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Protobuf.Schema</code> from <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">.proto</code> source. Pass a single string, or a <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">{`{ filename: source }`}</code> map for multi-file schemas with imports.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { Protobuf } from "runtime:serialization";

const schema = new Protobuf.Schema(\`
  syntax = "proto3";
  package shop;
  message Book {
    string id = 1;
    double price = 5;
    repeated string tags = 8;
  }
\`);`} title="protobuf_schema.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Encoding and decoding
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.encode</code> and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.decode</code> with a fully-qualified message name.
      </p>
      <div className="mt-6">
        <CodeBlock code={`const bytes = schema.encode("shop.Book", {
  id: "bk1",
  price: 44.95,
  tags: ["computer", "xml"],
});

const book = schema.decode("shop.Book", bytes);
console.log(book.price); // 44.95
console.log(book.tags);  // ["computer", "xml"]`} title="protobuf_roundtrip.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Decoded value shape
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Decoded objects use camelCase field names (or the explicit <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">json_name</code>). 64-bit integer fields surface as <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">BigInt</code>; enums as their value-name string; <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">bytes</code> as <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Uint8Array</code>; maps as plain objects. Fields absent on the wire are omitted.
      </p>
      <div className="mt-6">
        <CodeBlock code={`const schema = new Protobuf.Schema(\`
  syntax = "proto3";
  enum Status { ACTIVE = 0; ARCHIVED = 1; }
  message Account { uint64 id = 1; Status status = 2; }
\`);

const bytes = schema.encode("Account", { id: 9007199254740993n, status: "ARCHIVED" });
const acct = schema.decode("Account", bytes);
console.log(typeof acct.id);  // "bigint"
console.log(acct.status);     // "ARCHIVED"`} title="protobuf_types.js" lang="js" />
      </div>
    </DocsShell>
  );
}
