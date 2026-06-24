import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

// Official protobuf conformance suite (v29.3, --maximum_edition 2023
// --enforce_recommended) against our reflective codec. Binary and proto3-JSON
// are covered; the skips are text-format / JSPB and proto2-syntax cases. The
// single failure is a proto2 extension in JSON — unsupported by design and
// listed as an expected failure.
const CONFORMANCE = [
  { suite: "proto3", pass: 1413, skip: 396, fail: 0 },
  { suite: "proto2", pass: 0, skip: 1280, fail: 0 },
  { suite: "editions 2023", pass: 14, skip: 15, fail: 0 },
  { suite: "editions (proto3)", pass: 1413, skip: 396, fail: 0 },
  { suite: "editions (proto2)", pass: 1261, skip: 18, fail: 1 },
  { suite: "Total", pass: 4101, skip: 2105, fail: 1, total: true },
];

// The same run, split by wire format: binary and JSON are covered; text-format
// and JSPB are out of scope, as are the proto2-syntax binary cases (684 skips).
const CONFORMANCE_FMT = [
  { suite: "Binary", pass: 2060, skip: 684, fail: 0 },
  { suite: "JSON", pass: 2041, skip: 578, fail: 1 },
  { suite: "Text format", pass: 0, skip: 843, fail: 0 },
  { suite: "Total", pass: 4101, skip: 2105, fail: 1, total: true },
];

function ConformanceTable({ label, rows, firstCol }) {
  return (
    <div className="mt-6">
      <p className="mb-2 text-xs font-medium uppercase tracking-wide text-zinc-400">{label}</p>
      <div className="overflow-hidden rounded-xl border border-zinc-200">
        <table className="w-full text-left text-sm">
          <thead className="bg-zinc-50 text-zinc-600">
            <tr>
              <th className="px-4 py-3 font-medium">{firstCol}</th>
              <th className="px-3 py-3 text-right font-medium">Passed</th>
              <th className="px-3 py-3 text-right font-medium">Skipped</th>
              <th className="px-4 py-3 text-right font-medium">Failed</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-100">
            {rows.map((r) => (
              <tr className={r.total ? "bg-zinc-50/60 font-medium" : "hover:bg-zinc-50/60"}>
                <td className="px-4 py-2.5 text-zinc-700">{r.suite}</td>
                <td className="px-3 py-2.5 text-right font-mono text-brand-700">{r.pass.toLocaleString()}</td>
                <td className="px-3 py-2.5 text-right font-mono text-zinc-500">{r.skip.toLocaleString()}</td>
                <td className="px-4 py-2.5 text-right font-mono text-zinc-700">{r.fail}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}

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

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        JSON mapping
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.toJson</code> and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">schema.fromJson</code> convert between the decoded value shape and canonical proto3-JSON: 64-bit integers and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">bytes</code> become strings (base64 for bytes), enums their value-name, and the well-known types take their special forms (Timestamp/Duration as strings, wrappers as bare values, Struct/Value as native JSON, Any with an <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">@type</code> member). Parsing is strict; pass <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">{`{ ignoreUnknownFields: true }`}</code> to relax it.
      </p>
      <div className="mt-6">
        <CodeBlock code={`const json = schema.toJson("Account", schema.decode("Account", bytes));
// { "id": "9007199254740993", "status": "ARCHIVED" }

const value = schema.fromJson("Account", json);
const back = schema.encode("Account", value);`} title="protobuf_json.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Conformance
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Verified against the official protobuf conformance suite (v29.3). Binary and proto3-JSON both pass; JSPB, text-format, and proto2-syntax cases are reported as <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">skipped</code>. The one failure is a proto2 extension in JSON — unsupported by design.
      </p>
      <ConformanceTable label="By message category" firstCol="Category" rows={CONFORMANCE} />
      <ConformanceTable label="By wire format" firstCol="Wire format" rows={CONFORMANCE_FMT} />
    </DocsShell>
  );
}
