// Flattens JSONTestSuite's test_parsing and json5-tests into cases.json, the
// one file the Rust tests read. Run from the repository root:
//
//   esdev crates/json/test-suites/generate.js "$PWD/target/JSONTestSuite" "$PWD/target/json5-tests"
//
// after cloning the commits README.md names into target/. The checkouts must
// sit inside the repository: the runtime jails file access to it.
//
// Each case records what the engine's `JSON.parse` makes of the text, so the
// Rust side checks JSON values against JavaScript, not against itself. Nothing
// here evaluates the downloaded text as code: json5-tests' own runner checks
// its `.json5` values with `eval`, and those are covered by acceptance here and
// by the crate's unit tests for values.
import { file, readDir, write } from "runtime:fs";
import { args } from "runtime:process";

const [jsonTestSuite, json5Tests] = args;
if (!jsonTestSuite || !json5Tests) {
  throw new Error("usage: generate.js <JSONTestSuite checkout> <json5-tests checkout>");
}

// A value as JSON that serde_json reads back without loss: a number as the
// hex of its float64 bits (keeps -0, NaN and the infinities), a string with a
// lone surrogate as its UTF-16 code units, an object as its [key, value] pairs
// in the engine's own property order.
const encode = (value) => {
  if (value === null || typeof value === "boolean") return value;
  if (typeof value === "number") {
    const view = new DataView(new ArrayBuffer(8));
    view.setFloat64(0, value);
    return { n: view.getBigUint64(0).toString(16).padStart(16, "0") };
  }
  if (typeof value === "string") {
    return value.isWellFormed() ? value : { u16: Array.from(value, (_, i) => value.charCodeAt(i)).slice(0, value.length) };
  }
  if (Array.isArray(value)) return value.map(encode);
  return { o: Object.keys(value).map((key) => [encode(key), encode(value[key])]) };
};

const attempt = (parse, text) => {
  try {
    // As text, since serde_json reads at most 128 levels and a case nests 500.
    return { accepts: true, value: JSON.stringify(encode(parse(text))) };
  } catch {
    return { accepts: false };
  }
};

const strict = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
const decode = (bytes) => {
  try {
    return strict.decode(bytes);
  } catch {
    return null;
  }
};

const names = async (dir) => {
  const out = [];
  for (const entry of await readDir(dir)) out.push(entry);
  return out.sort((a, b) => (a.name < b.name ? -1 : 1));
};

const cases = [];
let skipped = 0;

// JSONTestSuite: y_ must parse, n_ must not, i_ is the implementation's call.
for (const entry of await names(`${jsonTestSuite}/test_parsing`)) {
  const text = decode(await file(`${jsonTestSuite}/test_parsing/${entry.name}`).bytes());
  // A JavaScript string cannot hold bytes that are not UTF-8, so these cases
  // test the decoder in front of the parser, not the parser.
  if (text === null) {
    skipped++;
    continue;
  }
  const kind = { y: "valid", n: "invalid", i: "either" }[entry.name[0]];
  cases.push({ suite: "JSONTestSuite", name: entry.name, kind, text, json: attempt(JSON.parse, text) });
}

// json5-tests: .json and .json5 must parse, .js and .txt must not. todo/ holds
// valid JSON5 the reference could not yet read. An errorSpec is a JSON5 object
// with numeric fields; only its line and column are read, by pattern.
const position = (spec) => {
  const line = /lineNumber:\s*(\d+)/.exec(spec);
  const column = /columnNumber:\s*(\d+)/.exec(spec);
  return line && column ? { line: Number(line[1]), column: Number(column[1]) } : null;
};
for (const dir of await names(json5Tests)) {
  if (!dir.isDir || dir.name.startsWith(".")) continue;
  for (const entry of await names(`${json5Tests}/${dir.name}`)) {
    const match = /^(.*)\.(json|json5|js|txt)$/.exec(entry.name);
    if (!match) continue;
    const path = `${json5Tests}/${dir.name}/${entry.name}`;
    const text = decode(await file(path).bytes());
    const kind = match[2] === "json" || match[2] === "json5" ? "valid" : "invalid";
    let error = null;
    try {
      error = position(await file(`${json5Tests}/${dir.name}/${match[1]}.errorSpec`).text());
    } catch {}
    cases.push({
      suite: "json5-tests",
      name: `${dir.name}/${entry.name}`,
      kind,
      text,
      json: attempt(JSON.parse, text),
      error,
    });
  }
}

await write(new URL("cases.json", import.meta.url).pathname, JSON.stringify(cases, null, 1) + "\n");
console.log(`${cases.length} cases, ${skipped} skipped as not UTF-8`);
