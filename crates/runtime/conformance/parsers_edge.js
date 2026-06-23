// Edge-case coverage for the runtime:parsers text + binary parsers, beyond the
// happy-path round-trips in parsers.js. Each test() is one tallied assertion.

// Order-insensitive deep equality via sorted-key JSON (function decl so loading
// this file twice in the shared realm can't trip a const redeclare).
function peq(actual, expected, msg) {
  function sortKeys(o) {
    if (o === null || typeof o !== "object") return o;
    if (Array.isArray(o)) return o.map(sortKeys);
    return Object.keys(o).sort().reduce((a, k) => ((a[k] = sortKeys(o[k])), a), {});
  }
  const a = JSON.stringify(sortKeys(actual));
  const e = JSON.stringify(sortKeys(expected));
  if (a !== e) throw new Error(`${msg}: expected ${e}, got ${a}`);
}

test("yaml edge cases", async () => {
  const { YAML } = await import("runtime:parsers");

  // Empty document is null; explicit null and `~` both parse to null.
  if (YAML.parse("") !== null) throw new Error("empty YAML should be null");
  peq(YAML.parse("a: null\nb: ~"), { a: null, b: null }, "yaml nulls");

  // Deep nesting (maps of arrays of maps) and anchors/aliases resolve.
  peq(YAML.parse("a:\n  b:\n    - 1\n    - x: 2"), { a: { b: [1, { x: 2 }] } }, "yaml nested");
  peq(YAML.parse("a: &x 1\nb: *x"), { a: 1, b: 1 }, "yaml anchors resolve");

  // Quoted scalars stay strings, even when they look like numbers/bools.
  peq(YAML.parse("a: '123'\nb: \"true\""), { a: "123", b: "true" }, "yaml quoted scalars");

  // Non-finite floats survive (regression guard; JSON can't carry these).
  const nf = YAML.parse("p: .inf\nn: -.inf\nx: .nan");
  if (nf.p !== Infinity || nf.n !== -Infinity || !Number.isNaN(nf.x)) {
    throw new Error("yaml non-finite floats lost");
  }

  // Build emits integers as integers, not 1.0; floats keep their point.
  if (YAML.build({ a: 1, b: 2.5 }) !== "a: 1\nb: 2.5\n") {
    throw new Error("yaml build integer fidelity: " + JSON.stringify(YAML.build({ a: 1, b: 2.5 })));
  }

  // Multi-document streams are unsupported and must throw, not silently drop docs.
  assertThrows(() => YAML.parse("---\na: 1\n---\nb: 2"));
});

test("toml edge cases", async () => {
  const { TOML } = await import("runtime:parsers");

  if (Object.keys(TOML.parse("")).length !== 0) throw new Error("empty TOML should be {}");

  // Arrays of tables, inline tables, and dotted/nested tables.
  peq(TOML.parse("[[p]]\nx=1\n[[p]]\nx=2"), { p: [{ x: 1 }, { x: 2 }] }, "toml array of tables");
  peq(TOML.parse("p = { a = 1, b = [2, 3] }"), { p: { a: 1, b: [2, 3] } }, "toml inline table");
  peq(TOML.parse("[a.b.c]\nx = 1"), { a: { b: { c: { x: 1 } } } }, "toml dotted table");

  // Datetimes / dates / times come back as strings (no $__toml_private_datetime).
  peq(
    TOML.parse("dt = 1979-05-27T07:32:00Z\nd = 1979-05-27\nt = 07:32:00"),
    { dt: "1979-05-27T07:32:00Z", d: "1979-05-27", t: "07:32:00" },
    "toml temporal types as strings"
  );

  // Build integer fidelity (b = 1, not 1.0) and float preserved.
  if (TOML.build({ a: 1, b: 1.5 }) !== "a = 1\nb = 1.5\n") {
    throw new Error("toml build integer fidelity: " + JSON.stringify(TOML.build({ a: 1, b: 1.5 })));
  }

  // A non-object root and a null value (TOML has no null) are rejected.
  assertThrows(() => TOML.build([1, 2, 3]));
  assertThrows(() => TOML.build({ a: null }));
});

test("messagepack edge cases", async () => {
  const { MessagePack } = await import("runtime:parsers");

  // Nested round-trip preserving null, bool, arrays, nested objects.
  const obj = { a: [1, 2, { b: null }], c: true, d: { e: "x" } };
  peq(MessagePack.decode(MessagePack.encode(obj)), obj, "msgpack nested round-trip");

  // Integers encode compactly (positive fixint), not as a 9-byte float64.
  const enc = MessagePack.encode({ n: 1 });
  if (enc.length !== 4) throw new Error("msgpack int should be compact, got " + enc.length + " bytes");

  // Empty map (0x80) decodes to {}; validate distinguishes good vs bad bytes.
  peq(MessagePack.decode(new Uint8Array([0x80])), {}, "msgpack empty map");
  if (MessagePack.validate(MessagePack.encode({ a: 1 })) !== true) throw new Error("valid msgpack");
  if (MessagePack.validate(new Uint8Array([0xc1])) !== false) throw new Error("0xc1 is never valid");
});

test("jsonl decoder round-trips, buffers partial lines, skips blanks", async () => {
  const { JSONL } = await import("runtime:parsers");
  const dec = new JSONL.DecoderStream();
  const writer = dec.writable.getWriter();
  const reader = dec.readable.getReader();
  const out = [];
  const drain = (async () => {
    for (;;) { const { done, value } = await reader.read(); if (done) break; out.push(value); }
  })();
  await writer.write('{"a":1}\n\n{"b":2}\n'); // blank line skipped
  await writer.write('{"c":');               // partial line buffered across writes
  await writer.write("3}\n");
  await writer.close();
  await drain;
  peq(out, [{ a: 1 }, { b: 2 }, { c: 3 }], "jsonl decode");
});

test("jsonl decoder handles a multi-byte char split across chunks", async () => {
  const { JSONL } = await import("runtime:parsers");
  const dec = new JSONL.DecoderStream();
  const writer = dec.writable.getWriter();
  const reader = dec.readable.getReader();
  const out = [];
  const drain = (async () => {
    for (;;) { const { done, value } = await reader.read(); if (done) break; out.push(value); }
  })();
  const bytes = new TextEncoder().encode('{"s":"€"}\n'); // € is 3 bytes (e2 82 ac)
  await writer.write(bytes.slice(0, 5)); // splits the euro sign mid-character
  await writer.write(bytes.slice(5));
  await writer.close();
  await drain;
  if (out.length !== 1 || out[0].s !== "€") throw new Error("euro split: " + JSON.stringify(out));
});

test("jsonl decoder skipInvalid tolerates and reports bad lines", async () => {
  const { JSONL } = await import("runtime:parsers");
  const dec = new JSONL.DecoderStream({ skipInvalid: true });
  const errs = [];
  dec.onError((e) => errs.push(e.line));
  const writer = dec.writable.getWriter();
  const reader = dec.readable.getReader();
  const out = [];
  const drain = (async () => {
    for (;;) { const { done, value } = await reader.read(); if (done) break; out.push(value); }
  })();
  await writer.write('{"a":1}\nNOTJSON\n{"b":2}\n');
  await writer.close();
  await drain;
  peq(out, [{ a: 1 }, { b: 2 }], "skipInvalid keeps good lines");
  if (errs.length !== 1 || errs[0] !== 2) throw new Error("expected bad line 2, got " + JSON.stringify(errs));
});

test("jsonl encoder produces ndjson", async () => {
  const { JSONL } = await import("runtime:parsers");
  const enc = new JSONL.EncoderStream();
  const writer = enc.writable.getWriter();
  const reader = enc.readable.getReader();
  let s = "";
  const drain = (async () => {
    for (;;) { const { done, value } = await reader.read(); if (done) break; s += value; }
  })();
  await writer.write({ a: 1 });
  await writer.write({ b: 2 });
  await writer.close();
  await drain;
  assertEquals(s, '{"a":1}\n{"b":2}\n');
});
