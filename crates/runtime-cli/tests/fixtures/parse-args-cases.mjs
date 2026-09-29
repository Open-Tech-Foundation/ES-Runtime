import { parseArgs } from "runtime:process";
// Tokenizer and validation cases for `parseArgs`. The expected output beside it
// (`parse-args-cases.expected`) was recorded from Node 24's `util.parseArgs` on
// this same script, with one deliberate difference on the last line: a
// non-string in `args` is ERR_INVALID_ARG_TYPE here (DECISIONS D142).
const O = { f: { type: "boolean", short: "f" }, name: { type: "string", short: "n" }, v: { type: "boolean", short: "v", multiple: true }, tag: { type: "string", multiple: true, short: "t", default: ["x"] }, color: { type: "boolean", default: true } };
const cases = [
  [["--name", "a", "-f"], { options: O }],
  [["--name=a=b", "-fvv", "-nfoo"], { options: O }],
  [["-fvnbar"], { options: O }],
  [["-t", "a", "--tag=b", "pos", "--", "--name"], { options: O, allowPositionals: true }],
  [["--no-color"], { options: O, allowNegative: true }],
  [["--no-name"], { options: O, allowNegative: true }],
  [["--unknown"], { options: O }],
  [["--unknown"], { options: O, allowPositionals: true }],
  [["pos"], { options: O }],
  [["--name"], { options: O }],
  [["--name", "--f"], { options: O }],
  [["-n", "-f"], { options: O }],
  [["--f=1"], { options: O }],
  [["--anything", "x", "-zq"], { strict: false }],
  [["--name", "a", "b", "--", "c"], { options: O, allowPositionals: true, tokens: true }],
  [["-fvv", "-nx"], { options: O, tokens: true }],
  [["--__proto__=1"], { strict: false }],
  [[], { options: { a: { type: "number" } } }],
  [[], { options: { a: { type: "string", short: "ab" } } }],
  [[], { options: { a: { type: "string", default: 1 } } }],
  [[], { options: { a: { type: "string", multiple: true, default: ["a", 2] } } }],
  [[1], {}],
];
const out = [];
for (const [args, config] of cases) {
  try {
    const r = parseArgs({ ...config, args });
    out.push(JSON.stringify({ values: { ...r.values }, positionals: r.positionals, tokens: r.tokens, proto: Object.getPrototypeOf(r.values) }));
  } catch (e) {
    out.push(JSON.stringify({ name: e.name, code: e.code }));
  }
}
console.log(out.join("\n"));
