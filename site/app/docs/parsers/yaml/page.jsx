import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function YAMLParserDoc() {
  return (
    <DocsShell active="/docs/parsers/yaml">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        YAML Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime provides high-performance native YAML parsing via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:parsers</code> module, backed by <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">serde_yaml</code>.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Parsing YAML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">YAMLParser.parse</code> to convert a YAML string directly into a JavaScript object.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { YAMLParser } from "runtime:parsers";

const yamlData = \`
user:
  id: 1
  name: Alice
\`;

const parsed = YAMLParser.parse(yamlData);
console.log(parsed.user.name); // "Alice"
console.log(parsed.user.id);   // 1`} title="yaml_parse.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Validating YAML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">YAMLValidator.validate</code> to check if a YAML string is well-formed.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { YAMLValidator } from "runtime:parsers";

const yamlData = \`
user:
  id: 1
  name: Alice
\`;

if (YAMLValidator.validate(yamlData)) {
  console.log("YAML is valid!");
}

const result = YAMLValidator.validate("invalid: \\n  - yaml: [", { detailed: true });
console.log(result.valid); // false
console.log(result.error); // "Validation failed: ..." `} title="yaml_validate.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Building YAML
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">YAMLBuilder.build</code> to convert a JavaScript object back into a YAML string.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { YAMLBuilder } from "runtime:parsers";

const obj = { 
  user: { 
    id: 1, 
    name: "Alice" 
  } 
};

const built = YAMLBuilder.build(obj);
console.log(built); 
// user:
//   id: 1
//   name: Alice`} title="yaml_build.js" lang="js" />
      </div>
    </DocsShell>
  );
}
