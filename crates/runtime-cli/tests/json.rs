//! End-to-end tests for `JSONC` and `JSON5` in `runtime:serialization`
//! (DECISIONS D191).
//!
//! The grammar is checked against JSONTestSuite and json5-tests by
//! `es-runtime-json`'s own tests; these cover the JavaScript side: that values
//! are what `JSON.parse` builds, that `reviver` and `stringify` behave as
//! `JSON`'s do, and the errors. Where the behaviour is meant to be `JSON`'s, the
//! test compares against the engine's own `JSON` rather than a hand-written
//! expectation.

use std::path::PathBuf;
use std::process::{Command, Output};

mod common;

fn write(name: &str, contents: &str) -> PathBuf {
    let path = common::tmpdir().join(name);
    std::fs::write(&path, contents).expect("write temp file");
    path
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs `source` with no grants at all and returns its stdout lines: the
/// formats read and reach nothing, so they need no capability.
fn run(name: &str, source: &str) -> Vec<String> {
    let script = format!("import {{ JSONC, JSON5 }} from \"runtime:serialization\";\n{source}");
    let app = write(name, &script);
    let out = Command::new(common::esrun_bin())
        .current_dir(common::tmpdir())
        .arg(&app)
        .output()
        .unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn both_read_json_exactly_as_json_parse_does() {
    let out = run(
        "json_same_as_json_parse.mjs",
        r#"
// A value's shape, including what JSON.stringify hides: -0, key order,
// own-versus-inherited, and lone surrogates.
const shape = (v) => {
  if (typeof v === "number") return Object.is(v, -0) ? "-0" : String(v);
  if (typeof v === "string") return JSON.stringify(v);
  if (v === null || typeof v !== "object") return String(v);
  const proto = Object.getPrototypeOf(v) === (Array.isArray(v) ? Array.prototype : Object.prototype);
  const items = Reflect.ownKeys(v).filter((k) => k !== "length").map((k) => `${String(k)}=${shape(v[k])}`);
  return `${Array.isArray(v) ? "A" : "O"}${proto ? "" : "!proto"}(${items.join(",")})`;
};
const texts = [
  '{"b":1,"a":2,"10":3,"2":4,"-1":5,"01":6}',
  '{"a":1,"b":2,"a":3}',
  '{"__proto__":{"polluted":true},"constructor":1}',
  '[-0, 0, 1e400, -1e-400, 1.7976931348623157e308, 5e-324, 123456789012345678901234567890, 0.1]',
  '"\\ud800\\udc00 \\ud800 \\udc00 \\u0000 \\u2028 é 😀"',
  '{"":{"":[]},"a b":[[],{}]}',
  'null', 'true', ' "x" ', '[1,[2,[3,[4]]]]',
];
for (const t of texts) {
  const want = shape(JSON.parse(t));
  console.log(shape(JSONC.parse(t)) === want, shape(JSON5.parse(t)) === want);
}
console.log(({}).polluted, Object.getPrototypeOf(JSONC.parse('{"__proto__":null}')) === Object.prototype);
// A string argument is coerced as JSON.parse coerces it.
console.log(JSONC.parse(123), JSON5.parse({ toString: () => "[1]" })[0]);
"#,
    );
    let mut want = vec!["true true"; 10];
    want.extend(["undefined true", "123 1"]);
    assert_eq!(out, want);
}

#[test]
fn jsonc_reads_a_commented_config_and_json5_reads_its_syntax() {
    let out = run(
        "json_config.mjs",
        r#"
const tsconfig = `﻿{
  // Visit https://aka.ms/tsconfig to read more about this file
  "compilerOptions": {
    /* Language and Environment */
    "target": "ES2022",                 // the emitted language
    "lib": ["ES2022", "DOM",],
    "paths": { "@app/*": ["./src/*"] },
  },
}`;
console.log(JSON.stringify(JSONC.parse(tsconfig)));
const renovate = `{
  // JSON5: a renovate.json5
  extends: ['config:recommended'],
  packageRules: [{ matchUpdateTypes: ['minor', 'patch',], automerge: true, },],
  schedule: "before 6am on monday",
  hex: 0x1F, half: .5, big: +1e3, nan: NaN, inf: -Infinity,
  text: 'it\\'s a \\
continued line',
}`;
const r = JSON5.parse(renovate);
console.log(JSON.stringify(r), r.nan, r.inf);
"#,
    );
    assert_eq!(
        out,
        [
            r#"{"compilerOptions":{"target":"ES2022","lib":["ES2022","DOM"],"paths":{"@app/*":["./src/*"]}}}"#,
            r#"{"extends":["config:recommended"],"packageRules":[{"matchUpdateTypes":["minor","patch"],"automerge":true}],"schedule":"before 6am on monday","hex":31,"half":0.5,"big":1000,"nan":null,"inf":null,"text":"it's a continued line"} NaN -Infinity"#,
        ]
    );
}

#[test]
fn reviver_runs_as_json_parse_runs_it() {
    let out = run(
        "json_reviver.mjs",
        r#"
const text = '{"a":[1,{"b":2}],"c":"x","d":{"e":null}}';
const log = (calls) => function (key, value) {
  calls.push([typeof this, Array.isArray(this), key, JSON.stringify(value)]);
  if (key === "c") return undefined;      // removed
  if (key === "0") return "first";        // replaced inside an array
  return value;
};
const want = []; const json = JSON.parse(text, log(want));
for (const parse of [JSONC.parse, JSON5.parse]) {
  const got = []; const value = parse(text, log(got));
  console.log(JSON.stringify(got) === JSON.stringify(want), JSON.stringify(value) === JSON.stringify(json), "c" in value);
}
// A reviver that is not a function is ignored, as JSON.parse ignores it.
console.log(JSONC.parse("[1]", 5)[0]);
"#,
    );
    assert_eq!(out, ["true true false", "true true false", "1"]);
}

#[test]
fn errors_name_the_dialect_and_the_place() {
    let out = run(
        "json_errors.mjs",
        r#"
const report = (f) => { try { f(); console.log("no error"); } catch (e) { console.log(`${e.name}: ${e.message}`); } };
report(() => JSONC.parse('{\n  name: "x"\n}'));
report(() => JSONC.parse("[1, 2,,]"));
report(() => JSONC.parse("{} /* open"));
report(() => JSONC.parse("'single'"));
report(() => JSON5.parse("{a: 01}"));
report(() => JSON5.parse("[1 2]"));
report(() => JSON5.parse(""));
report(() => JSON5.parse("[".repeat(257) + "]".repeat(257)));
console.log(JSONC.validate("[1,]"), JSONC.validate("[1,,]"), JSON5.validate("{a:1}"), JSONC.validate("{a:1}"));
console.log(JSON.stringify(JSON5.validate("'open", { detailed: true })), JSON.stringify(JSONC.validate("{}", { detailed: true })));
"#,
    );
    assert_eq!(
        out,
        [
            "SyntaxError: JSONC: a property name must be a double-quoted string at line 2 column 3",
            "SyntaxError: JSONC: expected a value, found ',' at line 1 column 7",
            "SyntaxError: JSONC: a comment is not closed at line 1 column 4",
            "SyntaxError: JSONC: expected a value, found ''' at line 1 column 1",
            "SyntaxError: JSON5: invalid number at line 1 column 5",
            "SyntaxError: JSON5: expected ',' or ']', found '2' at line 1 column 4",
            "SyntaxError: JSON5: unexpected end of the text at line 1 column 1",
            "RangeError: JSON5: the text nests more than 256 levels deep at line 1 column 257",
            "true false true false",
            r#"{"valid":false,"error":"JSON5: a string is not closed at line 1 column 1"} {"valid":true}"#,
        ]
    );
}

#[test]
fn jsonc_stringify_is_json_stringify() {
    let out = run(
        "json_jsonc_stringify.mjs",
        r#"
const v = { a: [1, undefined, () => 1], b: new Date(0), c: "é\u2028", d: { toJSON: () => "t" } };
for (const args of [[v], [v, null, 2], [v, ["a", "c"]], [v, (k, x) => (k === "c" ? undefined : x), "\t"], [undefined]]) {
  console.log(JSONC.stringify(...args) === JSON.stringify(...args));
}
"#,
    );
    assert_eq!(out, ["true"; 5]);
}

#[test]
fn json5_stringify_writes_json5_and_reads_back() {
    let out = run(
        "json_json5_stringify.mjs",
        r#"
const value = {
  name: "esrun", "kebab-key": 1, "": 0, "0": "zero", ünï: "quoted", $_ok: true,
  numbers: [NaN, Infinity, -Infinity, -0, 1e21, 0.1],
  quotes: ["it's", 'say "hi"', `both ' "`, "\0", "\x001", "\u2028", "\ud800", "😀"],
  nested: { empty: {}, list: [] },
};
console.log(JSON5.stringify(value));
console.log(JSON5.stringify({ a: [1, { b: 2 }], c: "x" }, null, 2));
console.log(JSON5.stringify({ a: "x", b: "it's" }, { quote: '"', space: 1 }));
// What JSON5 writes, JSON5 reads back as the same value.
const back = JSON5.parse(JSON5.stringify(value));
const same = (a, b) => Object.is(a, b) || (typeof a === "object" && a && b && Object.keys(a).length === Object.keys(b).length
  && Object.keys(a).every((k, i) => Object.keys(b)[i] === k && same(a[k], b[k])));
console.log(same(back, { ...value, numbers: [NaN, Infinity, -Infinity, 0, 1e21, 0.1] }));
"#,
    );
    assert_eq!(
        out,
        [
            r#"{'0':'zero',name:'esrun','kebab-key':1,'':0,'ünï':'quoted',$_ok:true,numbers:[NaN,Infinity,-Infinity,0,1e+21,0.1],quotes:["it's",'say "hi"','both \' "','\0','\x001','\u2028','\ud800','😀'],nested:{empty:{},list:[]}}"#,
            "{",
            "  a: [",
            "    1,",
            "    {",
            "      b: 2,",
            "    },",
            "  ],",
            "  c: 'x',",
            "}",
            "{",
            r#" a: "x","#,
            r#" b: "it's","#,
            "}",
            "true",
        ]
    );
}

#[test]
fn json5_stringify_follows_json_stringify_on_what_it_skips_and_refuses() {
    let out = run(
        "json_json5_stringify_rules.mjs",
        r#"
const report = (f) => { try { console.log(f()); } catch (e) { console.log(`${e.name}: ${e.message}`); } };
report(() => JSON5.stringify({ u: undefined, f() {}, s: Symbol("s"), [Symbol("k")]: 1, list: [undefined, () => 1, Symbol()] }));
report(() => JSON5.stringify(undefined));
report(() => JSON5.stringify({ d: new Date(0), b: new Boolean(false), n: new Number(3), s: new String("x") }));
report(() => JSON5.stringify({ j: { toJSON: () => "json" }, j5: { toJSON5: () => "json5", toJSON: () => "json" } }));
report(() => JSON5.stringify({ a: 1, b: 2, c: { a: 3, b: 4 } }, ["a", "c"]));
report(() => JSON5.stringify({ a: 1, b: 2 }, function (k, v) { return k === "a" ? v * 10 : v; }));
report(() => JSON5.stringify({ big: 1n }));
report(() => { const a = {}; a.self = a; return JSON5.stringify(a); });
report(() => JSON5.stringify([1], { quote: "`" }));
report(() => JSON5.stringify([1], null, 20).split("\n")[1].length);
"#,
    );
    assert_eq!(
        out,
        [
            "{list:[null,null,null]}",
            "undefined",
            "{d:'1970-01-01T00:00:00.000Z',b:false,n:3,s:'x'}",
            "{j:'json',j5:'json5'}",
            "{a:1,c:{a:3}}",
            "{a:10,b:2}",
            "TypeError: JSON5.stringify: a BigInt has no JSON5 form",
            "TypeError: JSON5.stringify: the value is circular",
            r#"TypeError: JSON5.stringify: quote must be "'" or '"'"#,
            "12",
        ]
    );
}

#[test]
fn the_builtins_stay_hidden_and_the_default_export_has_both() {
    let out = run(
        "json_hidden.mjs",
        r#"
import serialization from "runtime:serialization";
console.log(Object.keys(globalThis).filter((k) => k.startsWith("__json")).length);
console.log(serialization.JSONC === JSONC, serialization.JSON5 === JSON5);
"#,
    );
    assert_eq!(out, ["0", "true true"]);
}
