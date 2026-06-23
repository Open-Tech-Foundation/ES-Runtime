import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

export default function MessagePackParserDoc() {
  return (
    <DocsShell active="/docs/serialization/msgpack">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        MessagePack Processing
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        ES-Runtime provides high-performance native MessagePack parsing via the <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:serialization</code> module, backed by <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">rmp-serde</code>.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Parsing MessagePack
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">MessagePack.decode</code> to convert a MessagePack byte array directly into a JavaScript object.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { MessagePack } from "runtime:serialization";

const msgpackBytes = new Uint8Array([0x81, 0xa4, 0x75, 0x73, 0x65, 0x72, 0x82, 0xa2, 0x69, 0x64, 0x01, 0xa4, 0x6e, 0x61, 0x6d, 0x65, 0xa5, 0x41, 0x6c, 0x69, 0x63, 0x65]);

const parsed = MessagePack.decode(msgpackBytes);
console.log(parsed.user.name); // "Alice"
console.log(parsed.user.id);   // 1`} title="msgpack_parse.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Validating MessagePack
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">MessagePack.validate</code> to check if a MessagePack byte array is well-formed.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { MessagePack } from "runtime:serialization";

const msgpackBytes = new Uint8Array([0x81, 0xa4, 0x75, 0x73, 0x65, 0x72, 0x82, 0xa2, 0x69, 0x64, 0x01, 0xa4, 0x6e, 0x61, 0x6d, 0x65, 0xa5, 0x41, 0x6c, 0x69, 0x63, 0x65]);

if (MessagePack.validate(msgpackBytes)) {
  console.log("MessagePack is valid!");
}

const invalidBytes = new Uint8Array([0xc1]);
const result = MessagePack.validate(invalidBytes, { detailed: true });
console.log(result.valid); // false
console.log(result.error); // "Validation failed: ..." `} title="msgpack_validate.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">
        Building MessagePack
      </h2>
      <p className="mt-2 text-zinc-600 leading-relaxed">
        Use <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">MessagePack.encode</code> to convert a JavaScript object back into a MessagePack byte array.
      </p>
      <div className="mt-6">
        <CodeBlock code={`import { MessagePack } from "runtime:serialization";

const obj = { 
  user: { 
    id: 1, 
    name: "Alice" 
  } 
};

const built = MessagePack.encode(obj);
console.log(built instanceof Uint8Array); // true`} title="msgpack_build.js" lang="js" />
      </div>
    </DocsShell>
  );
}
