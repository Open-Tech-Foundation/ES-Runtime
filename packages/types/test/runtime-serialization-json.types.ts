// A type test for `runtime:serialization`'s JSONC and JSON5 (D191). Compiled by `tsc -p .`, never run.

import { JSON5, JSONC } from "runtime:serialization";

const config: { compilerOptions?: object } = JSONC.parse('{ "compilerOptions": {}, // c\n }');
const revived: unknown = JSON5.parse("{a: 1}", (key, value) => (key === "a" ? value * 2 : value));
const json: string = JSONC.stringify({ a: 1 }, ["a"], 2);
const json5: string = JSON5.stringify({ a: 1 }, null, "\t");
const quoted: string = JSON5.stringify({ a: "x" }, { quote: '"', space: 2 });
const ok: boolean | { valid: boolean; error?: string } = JSON5.validate("{a:1}", {
  detailed: true,
});
const edited: string = JSONC.edit("{}", ["compilerOptions", "types"], ["x"]);
const removed: string = JSON5.edit("{a: [1]}", ["a", 0], undefined);
// @ts-expect-error — a path is an array of keys and indices.
JSONC.edit("{}", "a", 1);
// @ts-expect-error — a quote is ' or ".
JSON5.stringify({}, { quote: "`" });
// @ts-expect-error — a reviver is a function.
JSONC.parse("{}", 5);
void [config, revived, json, json5, quoted, ok, edited, removed];
