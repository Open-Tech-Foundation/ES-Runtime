// Runs MongoDB's BSON corpus and binary-vector tests against runtime:serialization's
// BSON, unmodified. Fetch them first with ./mongodb-specs/fetch.sh.
//
//   esrun --allow-read mongodb-specs/bson-corpus.js [-- --verbose]
//
// For each valid case the native round trip must give the canonical bytes back
// (decode then encode), from the degenerate bytes too; each decode error must
// be refused; Decimal128 text must parse and print canonically and its parse
// errors must be refused. Extended JSON is not implemented, so its cases are
// not run. Where JavaScript cannot represent a value the way BSON does, the
// case is listed in DIFFERENCES with the reason, and anything else that fails
// fails the run.
import { BSON } from "runtime:serialization";
import { args } from "runtime:process";
import { file, readDir } from "runtime:fs";

const verbose = args.includes("--verbose");
const root = new URL("./upstream/source/", import.meta.url).pathname;

// Cases whose canonical bytes do not come back, and why. Keyed `file: description`.
const DIFFERENCES = {
  // A whole-number double reads as a JavaScript number, which writes as int32,
  // as in the official driver. `BSON.Double` keeps a double.
  "double.json: -1.0": "an integral double re-encodes as int32",
  "double.json: +1.0": "an integral double re-encodes as int32",
  "double.json: 0.0": "an integral double re-encodes as int32",
  "multi-type.json: All BSON types": "its double -1.0 re-encodes as int32",
  "multi-type-deprecated.json: All BSON types": "its double -1.0 re-encodes as int32",
  "double.json: NaN with payload": "a NaN's payload bits do not survive a JavaScript number",
};
const used = new Set();

const results = { pass: 0, fail: 0, skip: 0, known: 0 };
const failures = [];

function bytes(hex) {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

function same(a, b) {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

function check(name, ok, detail = "") {
  if (ok) {
    if (Object.hasOwn(DIFFERENCES, name)) failures.push(`${name} — passes now; remove it from DIFFERENCES`);
    results.pass++;
    if (verbose) console.log(`  ok   ${name}`);
    return;
  }
  if (Object.hasOwn(DIFFERENCES, name)) {
    used.add(name);
    results.known++;
    if (verbose) console.log(`  diff ${name} — ${DIFFERENCES[name]}`);
    return;
  }
  results.fail++;
  failures.push(`${name}${detail ? ` — ${detail}` : ""}`);
}

function attempt(fn) {
  try {
    return { value: fn() };
  } catch (error) {
    return { error };
  }
}

async function json(path) {
  return JSON.parse(await file(path).text());
}

async function corpus() {
  const dir = `${root}bson-corpus/tests/`;
  for (const entry of (await readDir(dir)).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    if (!entry.name.endsWith(".json")) continue;
    const suite = await json(dir + entry.name);
    const decimal = suite.bson_type === "0x13";
    for (const c of suite.valid ?? []) {
      const name = `${entry.name}: ${c.description}`;
      const cB = bytes(c.canonical_bson);
      const decoded = attempt(() => BSON.decode(cB));
      if (decoded.error) {
        check(name, false, `decode: ${decoded.error.message}`);
        continue;
      }
      const expected = suite.deprecated ? (c.converted_bson && bytes(c.converted_bson)) : cB;
      if (expected) {
        const encoded = attempt(() => BSON.encode(decoded.value));
        check(name, !encoded.error && same(encoded.value, expected), encoded.error?.message ?? "bytes differ");
      } else {
        results.skip++;
      }
      if (c.degenerate_bson && !suite.deprecated) {
        const again = attempt(() => BSON.encode(BSON.decode(bytes(c.degenerate_bson))));
        check(`${name} (degenerate)`, !again.error && same(again.value, cB), again.error?.message ?? "bytes differ");
      }
      if (decimal) decimalText(name, c, decoded.value[suite.test_key]);
    }
    for (const c of suite.decodeErrors ?? []) {
      const name = `${entry.name}: ${c.description}`;
      const input = bytes(c.bson);
      const decoded = attempt(() => BSON.decode(input));
      check(name, decoded.error?.name === "SyntaxError", decoded.error ? decoded.error.message : "decoded");
      check(`${name} (validate)`, BSON.validate(input) === false, "validated");
    }
    for (const c of suite.parseErrors ?? []) {
      if (!decimal) {
        results.skip++; // Extended JSON parse errors.
        continue;
      }
      const parsed = attempt(() => new BSON.Decimal128(c.string));
      check(`${entry.name}: ${c.description}`, parsed.error !== undefined, "parsed");
    }
  }
}

// Decimal128 text: the canonical string prints from the bytes and parses back to
// them, and a degenerate spelling parses to the same bytes.
function decimalText(name, c, value) {
  const text = (ej) => JSON.parse(ej).d.$numberDecimal;
  const stored = value.bytes;
  check(`${name} (toString)`, value.toString() === text(c.canonical_extjson), value.toString());
  if (c.lossy) return;
  const parsed = attempt(() => new BSON.Decimal128(text(c.canonical_extjson)).bytes);
  check(`${name} (parse)`, !parsed.error && same(parsed.value, stored), parsed.error?.message ?? "bytes differ");
  if (c.degenerate_extjson) {
    const again = attempt(() => new BSON.Decimal128(text(c.degenerate_extjson)).bytes);
    check(`${name} (parse degenerate)`, !again.error && same(again.value, stored), again.error?.message ?? "bytes differ");
  }
}

const VECTOR = {
  "0x27": { make: (v) => BSON.Binary.fromFloat32Array(v.map(number)), read: (b) => [...b.toFloat32Array()] },
  "0x03": { make: (v) => BSON.Binary.fromInt8Array(v), read: (b) => [...b.toInt8Array()] },
  "0x10": { make: (v, p) => BSON.Binary.fromPackedBits(v, p), read: (b) => [...b.toPackedBits()] },
};

function number(v) {
  return typeof v === "object" ? Number(v.$numberDouble) : v;
}

async function vectors() {
  const dir = `${root}bson-binary-vector/tests/`;
  for (const entry of (await readDir(dir)).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    if (!entry.name.endsWith(".json")) continue;
    const suite = await json(dir + entry.name);
    for (const c of suite.tests) {
      const name = `${entry.name}: ${c.description}`;
      const kind = VECTOR[c.dtype_hex];
      const padding = c.padding ?? 0;
      if (c.valid) {
        const made = attempt(() => BSON.encode({ [suite.test_key]: kind.make(c.vector, padding) }));
        check(`${name} (encode)`, !made.error && same(made.value, bytes(c.canonical_bson)), made.error?.message ?? "bytes differ");
        const binary = BSON.decode(bytes(c.canonical_bson))[suite.test_key];
        const read = attempt(() => kind.read(binary));
        const want = c.vector.map((v) => Math.fround(number(v)));
        const got = read.value?.map((v) => (c.dtype_hex === "0x27" ? Math.fround(v) : v));
        check(`${name} (decode)`, !read.error && same(got, want) && binary.buffer[1] === padding, read.error?.message ?? String(got));
        continue;
      }
      // An invalid vector cannot be made, and invalid bytes cannot be read as one.
      // A padding the float32 and int8 constructors cannot express is invalid by
      // construction.
      if (c.vector) {
        const made = attempt(() => {
          if (padding !== 0 && c.dtype_hex !== "0x10") throw new RangeError("no padding");
          return kind.make(c.vector, padding);
        });
        check(`${name} (encode)`, made.error !== undefined, "made");
      }
      if (c.canonical_bson) {
        const read = attempt(() => kind.read(BSON.decode(bytes(c.canonical_bson))[suite.test_key]));
        check(`${name} (decode)`, read.error !== undefined, "read");
      }
    }
  }
}

await corpus();
await vectors();

// A listed difference that now passes is stale and should be removed.
console.log(
  `bson corpus: ${results.pass} passed, ${results.known} known differences, ${results.skip} not applicable, ${results.fail} failed`,
);

for (const name of Object.keys(DIFFERENCES)) {
  if (!used.has(name) && !failures.some((f) => f.startsWith(name))) failures.push(`${name} — not in the corpus; remove it`);
}
for (const f of failures) console.log(`  FAIL ${f}`);
if (failures.length > 0) throw new Error(`${failures.length} BSON corpus checks failed`);
