import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function TOMLParserDoc() {
  return (
    <DocsShell active="/docs/parsers/toml">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        TOML Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime provides high-performance native TOML parsing via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module, backed by the fast <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">toml</code> rust crate.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Parsing TOML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">TOMLParser.parse</code> to convert a TOML string directly into a JavaScript object.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { TOMLParser } from "runtime:parsers";

const tomlData = \`
[user]
id = 1
name = "Alice"
\`;

const parsed = TOMLParser.parse(tomlData);
console.log(parsed.user.name); // "Alice"
console.log(parsed.user.id);   // 1`} title="toml_parse.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Validating TOML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">TOMLValidator.validate</code> to check if a TOML string is well-formed.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { TOMLValidator } from "runtime:parsers";

const tomlData = \`
[user]
id = 1
name = "Alice"
\`;

if (TOMLValidator.validate(tomlData)) {
  console.log("TOML is valid!");
}

const result = TOMLValidator.validate("invalid = TOML [[", { detailed: true });
console.log(result.valid); // false
console.log(result.error); // "Validation failed: ..." `} title="toml_validate.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Building TOML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">TOMLBuilder.build</code> to convert a JavaScript object back into a TOML string. Note that the root of the JavaScript object must map to a TOML table (an object).
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { TOMLBuilder } from "runtime:parsers";

const obj = { 
  user: { 
    id: 1, 
    name: "Alice" 
  } 
};

const built = TOMLBuilder.build(obj);
console.log(built); 
// [user]
// id = 1
// name = "Alice"`} title="toml_build.js" lang="js" />
      </div>
    </DocsShell>
  );
}
