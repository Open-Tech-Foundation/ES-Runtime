test("parsers sync apis", async () => {
const { YAML, XML, TOML, MessagePack, JSONL } = await import('runtime:serialization');

function assertEq(actual, expected, msg) {
    function sortKeys(obj) {
        if (obj === null || typeof obj !== 'object') return obj;
        if (Array.isArray(obj)) return obj.map(sortKeys);
        return Object.keys(obj).sort().reduce((acc, key) => {
            acc[key] = sortKeys(obj[key]);
            return acc;
        }, {});
    }
    const actualStr = JSON.stringify(sortKeys(actual));
    const expectedStr = JSON.stringify(sortKeys(expected));
    if (actualStr !== expectedStr) {
        throw new Error(`${msg}: Expected ${expectedStr}, got ${actualStr}`);
    }
}

function assertThrows(fn, msg) {
    try {
        fn();
        throw new Error(`${msg}: Expected to throw, but did not`);
    } catch (e) {
        // Expected
    }
}

// YAML Parsing Tests
const yamlData = `
name: Alice
age: 30
is_active: true
roles:
  - admin
  - user
settings:
  theme: dark
`;

const expectedYamlParsed = {
    name: "Alice",
    age: 30,
    is_active: true,
    roles: ["admin", "user"],
    settings: {
        theme: "dark"
    }
};

assertEq(YAML.parse(yamlData), expectedYamlParsed, "YAML parsing basic");

// YAML Validation Tests
assertEq(YAML.validate(yamlData), true, "YAML validation valid");
assertEq(YAML.validate(yamlData, { detailed: true }), { valid: true }, "YAML validation valid detailed");

const invalidYaml = `
name: Alice
  age: 30
`;

assertEq(YAML.validate(invalidYaml), false, "YAML validation invalid");
const invalidDetailed = YAML.validate(invalidYaml, { detailed: true });
if (invalidDetailed.valid !== false || typeof invalidDetailed.error !== 'string') {
    throw new Error("YAML validation invalid detailed failed");
}

assertThrows(() => YAML.parse(invalidYaml), "YAML parse invalid throws");

// Non-finite floats must survive as Infinity/NaN, not be coerced to null.
const yamlNonFinite = YAML.parse("pos: .inf\nneg: -.inf\nnan: .nan");
if (yamlNonFinite.pos !== Infinity) throw new Error("YAML .inf should be Infinity");
if (yamlNonFinite.neg !== -Infinity) throw new Error("YAML -.inf should be -Infinity");
if (!Number.isNaN(yamlNonFinite.nan)) throw new Error("YAML .nan should be NaN");

// YAML Building Tests
const objToBuild = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtYaml = YAML.build(objToBuild);
if (!builtYaml.includes("Bob") || !builtYaml.includes("42")) {
    throw new Error("YAML build failed: " + builtYaml);
}
assertEq(YAML.parse(builtYaml), objToBuild, "YAML build back to obj");

console.log("YAML tests passed!");

// TOML Parsing Tests
const tomlData = `
name = "Alice"
age = 30
is_active = true
roles = ["admin", "user"]

[settings]
theme = "dark"
`;

const expectedTomlParsed = {
    name: "Alice",
    age: 30,
    is_active: true,
    roles: ["admin", "user"],
    settings: {
        theme: "dark"
    }
};

assertEq(TOML.parse(tomlData), expectedTomlParsed, "TOML parsing basic");

// TOML Validation Tests
assertEq(TOML.validate(tomlData), true, "TOML validation valid");
assertEq(TOML.validate(tomlData, { detailed: true }), { valid: true }, "TOML validation valid detailed");

const invalidToml = `
name = Alice
  age = 30
`;

assertEq(TOML.validate(invalidToml), false, "TOML validation invalid");
const tomlInvalidDetailed = TOML.validate(invalidToml, { detailed: true });
if (tomlInvalidDetailed.valid !== false || typeof tomlInvalidDetailed.error !== 'string') {
    throw new Error("TOML validation invalid detailed failed");
}

assertThrows(() => TOML.parse(invalidToml), "TOML parse invalid throws");

// Datetimes must come back as RFC3339 strings, not the toml-crate sentinel object.
assertEq(
    TOML.parse("dt = 1979-05-27T07:32:00Z\nld = 1979-05-27"),
    { dt: "1979-05-27T07:32:00Z", ld: "1979-05-27" },
    "TOML datetimes parse to strings"
);

// TOML Building Tests
const objToBuildToml = {
    user: {
        name: "Bob",
        id: 42
    }
};

const builtToml = TOML.build(objToBuildToml);
if (!builtToml.includes("Bob") || !builtToml.includes("42")) {
    throw new Error("TOML build failed: " + builtToml);
}
assertEq(TOML.parse(builtToml), objToBuildToml, "TOML build back to obj");

console.log("TOML tests passed!");

// MessagePack Tests
const objToBuildMsgpack = {
    user: {
        name: "Charlie",
        id: 99
    }
};

const builtMsgpack = MessagePack.encode(objToBuildMsgpack);
if (!(builtMsgpack instanceof Uint8Array)) {
    throw new Error("MessagePack encode did not return Uint8Array");
}
assertEq(MessagePack.decode(builtMsgpack), objToBuildMsgpack, "MessagePack decode back to obj");
assertEq(MessagePack.validate(builtMsgpack), true, "MessagePack validation valid");

// Invalid msgpack
const invalidMsgpack = new Uint8Array([0xc1, 0x01]); // 0xc1 is never used
assertEq(MessagePack.validate(invalidMsgpack), false, "MessagePack validation invalid");
const msgpackInvalidDetailed = MessagePack.validate(invalidMsgpack, { detailed: true });
if (msgpackInvalidDetailed.valid !== false || typeof msgpackInvalidDetailed.error !== 'string') {
    throw new Error("MessagePack validation invalid detailed failed");
}
assertThrows(() => MessagePack.decode(invalidMsgpack), "MessagePack decode invalid throws");

console.log("MessagePack tests passed!");
});

// ---- MessagePack binary fidelity -------------------------------------------
//
// The `bin` family is the reason to choose a binary format, and it used to be
// destroyed in both directions: `encode` wrote `nil` for a `Uint8Array` (the
// whole payload gone, silently) and `decode` returned a plain `Array` of
// numbers for a `bin` value, so nothing round-tripped and foreign MessagePack
// lost its type.
test("MessagePack round-trips binary data", async () => {
  const { MessagePack } = await import("runtime:serialization");
  const bytes = new Uint8Array([0, 1, 254, 255]);

  const encoded = MessagePack.encode(bytes);
  assertEquals(encoded[0], 0xc4, "a Uint8Array must encode as the bin family, not nil");
  const back = MessagePack.decode(encoded);
  assert(back instanceof Uint8Array, "bin must decode to a Uint8Array");
  assertEquals([...back].join(), [...bytes].join());

  // …nested, where the containing document is otherwise JSON-shaped.
  const doc = { name: "blob", data: new Uint8Array([9, 8, 7]), n: 1 };
  const rt = MessagePack.decode(MessagePack.encode(doc));
  assert(rt.data instanceof Uint8Array, "nested bin must survive as bytes");
  assertEquals([...rt.data].join(), "9,8,7");
  assertEquals(rt.name, "blob");
  assertEquals(rt.n, 1);
});

test("MessagePack decodes foreign bin and ext without flattening them", async () => {
  const { MessagePack } = await import("runtime:serialization");
  // Hand-built: bin8 of three bytes, and fixext1 of type 5.
  const bin = MessagePack.decode(new Uint8Array([0xc4, 0x03, 1, 2, 3]));
  assert(bin instanceof Uint8Array, "hand-built bin8 must decode to bytes");
  assertEquals([...bin].join(), "1,2,3");
  const ext = MessagePack.decode(new Uint8Array([0xd4, 0x05, 0x42]));
  assert(ext instanceof Uint8Array, "an ext payload must be kept");
  assertEquals([...ext].join(), "66");
});

test("MessagePack keeps values that carry no enumerable properties", async () => {
  const { MessagePack } = await import("runtime:serialization");
  const rt = (v) => MessagePack.decode(MessagePack.encode(v));
  // Each of these used to cross the boundary as `{}` and encode as an empty
  // map — every entry silently dropped.
  assertEquals(JSON.stringify(rt(new Map([["a", 1]]))), '{"a":1}');
  assertEquals(JSON.stringify(rt(new Set([1, 2]))), "[1,2]");
  assertEquals(rt(new Date(0)), "1970-01-01T00:00:00.000Z");
  const buf = rt(new Uint8Array([7]).buffer);
  assert(buf instanceof Uint8Array, "an ArrayBuffer must encode as bytes");
  assertEquals([...buf].join(), "7");
});

test("MessagePack refuses a value it cannot represent instead of writing nil", async () => {
  const { MessagePack } = await import("runtime:serialization");
  // Silently encoding these as nil is what made the data loss invisible.
  assertThrows(() => MessagePack.encode(() => {}), "TypeError");
  assertThrows(() => MessagePack.encode(1n), "TypeError");
});

test("MessagePack still round-trips the JSON-shaped documents on the fast path", async () => {
  const { MessagePack } = await import("runtime:serialization");
  // No bin anywhere, so this takes the JSON pivot; the numeric forms are the
  // ones the encoder picks per width.
  const doc = {
    s: "héllo 😀", t: true, f: false, nil: null,
    ints: [0, 127, 128, 255, 256, 65535, 65536, 4294967296, -1, -32, -33, -128, -32768, -2147483648],
    float: 1.5,
    nested: { a: [{ b: "c" }] },
  };
  assertEquals(JSON.stringify(MessagePack.decode(MessagePack.encode(doc))), JSON.stringify(doc));
  // A string long enough to leave the fixstr range, and an empty container.
  assertEquals(MessagePack.decode(MessagePack.encode("x".repeat(40000))).length, 40000);
  assertEquals(JSON.stringify(MessagePack.decode(MessagePack.encode({}))), "{}");
  assertEquals(JSON.stringify(MessagePack.decode(MessagePack.encode([]))), "[]");
});

// ---- CBOR ------------------------------------------------------------------
//
// CBOR (RFC 8949) joins as a second binary format. It shares MessagePack's
// byte-for-byte concerns but adds the format's own decisions: the deterministic
// encoding (shortest forms, canonical float width, sorted keys) on the way out,
// and tags refused on the way in.
test("CBOR round-trips the JSON-shaped documents and prefers integer forms", async () => {
  const { CBOR } = await import("runtime:serialization");
  // Canonical encoding sorts map keys, so an object comes back with its keys
  // reordered — compare order-insensitively.
  const sortKeys = (v) =>
    v !== null && typeof v === "object" && !Array.isArray(v)
      ? Object.fromEntries(Object.entries(v).sort(([a], [b]) => (a < b ? -1 : 1)).map(([k, x]) => [k, sortKeys(x)]))
      : Array.isArray(v)
        ? v.map(sortKeys)
        : v;
  const doc = {
    t: true, f: false, nil: null, n: -25, big: 4294967296,
    s: "héllo 😀", float: 1.5, arr: [1, 2, 3], nested: { a: "b" },
  };
  assertEquals(JSON.stringify(sortKeys(CBOR.decode(CBOR.encode(doc)))), JSON.stringify(sortKeys(doc)));
  // The shortest integer form, and major type 1 for negatives.
  assertEquals([...CBOR.encode(0)].join(), "0");
  assertEquals([...CBOR.encode(23)].join(), "23");
  assertEquals([...CBOR.encode(24)].join(), "24,24");
  assertEquals([...CBOR.encode(-1)].join(), "32");
  assertEquals([...CBOR.encode(-25)].join(), "56,24");
});

test("CBOR floats take the shortest exact width and special values are canonical", async () => {
  const { CBOR } = await import("runtime:serialization");
  assertEquals([...CBOR.encode(1.5)].join(), "249,62,0");
  assertEquals([...CBOR.encode(Infinity)].join(), "249,124,0");
  assertEquals([...CBOR.encode(-0.0)].join(), "249,128,0");
  // Hand-built f64 pi decodes exactly.
  assertEquals(
    CBOR.decode(new Uint8Array([0xfb, 0x40, 0x09, 0x21, 0xfb, 0x54, 0x44, 0x2d, 0x18])),
    3.141592653589793,
  );
});

test("CBOR sorts map keys canonically", async () => {
  const { CBOR } = await import("runtime:serialization");
  // By encoded bytes the length prefix sorts first, so "a" < "b" < "aa" — not
  // the insertion order and not plain lexicographic order.
  assertEquals(
    [...CBOR.encode({ b: 1, a: 2, aa: 3 })].join(),
    "163,97,97,2,97,98,1,98,97,97,3",
  );
});

test("CBOR keeps byte strings and reads indefinite-length input", async () => {
  const { CBOR } = await import("runtime:serialization");
  const back = CBOR.decode(CBOR.encode({ data: new Uint8Array([0, 1, 254, 255]) }));
  assert(back.data instanceof Uint8Array, "a byte string must decode to a Uint8Array");
  assertEquals([...back.data].join(), "0,1,254,255");
  // Indefinite-length text (_ "hi", "!!") and array (_ 1, 2).
  assertEquals(CBOR.decode(new Uint8Array([0x7f, 0x62, 0x68, 0x69, 0x62, 0x21, 0x21, 0xff])), "hi!!");
  assertEquals(JSON.stringify(CBOR.decode(new Uint8Array([0x9f, 0x01, 0x02, 0xff]))), "[1,2]");
});

test("CBOR refuses tags and malformed input, and validate agrees", async () => {
  const { CBOR } = await import("runtime:serialization");
  assertThrows(() => CBOR.decode(new Uint8Array([0xc0, 0x61, 0x61])), "SyntaxError"); // tag 0
  assertThrows(() => CBOR.decode(new Uint8Array([0x00, 0x00])), "SyntaxError"); // trailing byte
  assertThrows(() => CBOR.decode(new Uint8Array([0xff])), "SyntaxError"); // stray break
  assert(!CBOR.validate(new Uint8Array([0xc0, 0x61, 0x61])), "a tag is not valid");
  assert(CBOR.validate(new Uint8Array([0x00])), "a bare integer is valid");
  // A value with no CBOR form throws rather than being written as null.
  assertThrows(() => CBOR.encode(() => {}), "TypeError");
});

// ---- XML well-formedness ---------------------------------------------------
//
// EOF inside an open element used to end the parse quietly, so a truncated
// document produced a partial object instead of an error: `<r>` parsed to
// `{"r":{}}`, and `validate` agreed it was fine. A *mismatched* end tag was
// already caught, so only the truncated case got through.
test("XML rejects a document that ends with elements still open", async () => {
  const { XML } = await import("runtime:serialization");
  for (const src of ["<r>", "<r><a>", "<r><a>1</a>", "<r><a>1"]) {
    assertThrows(() => XML.parse(src), "SyntaxError");
    assertEquals(XML.validate(src), false, `validate ${JSON.stringify(src)}`);
  }
  for (const src of ["<r></x>", "</r>", '<r a="1>']) {
    assertThrows(() => XML.parse(src), "SyntaxError");
  }
  assertEquals(JSON.stringify(XML.parse("<r><a>1</a></r>")), '{"r":{"a":{"$text":"1"}}}');
  assertEquals(XML.validate("<r><a/><a/></r>"), true);
  assertEquals(XML.validate("<r><a>1</a></r>"), true);
});

test("YAML block scalars chomp per the spec", async () => {
  const { YAML } = await import("runtime:serialization");
  const a = (src) => YAML.parse(src).a;
  // Clip (the default) keeps exactly one trailing break when the source has
  // one, and none at EOF where there is nothing to keep.
  assertEquals(a("a: |\n  l1\n  l2\n"), "l1\nl2\n");
  assertEquals(a("a: >\n  l1\n  l2\n"), "l1 l2\n");
  assertEquals(a("a: |\n  l1\n  l2"), "l1\nl2");
  assertEquals(a("a: |\n  l1\n  l2\n\n\n"), "l1\nl2\n");
  // Strip removes it; keep retains every one.
  assertEquals(a("a: |-\n  l1\n  l2\n"), "l1\nl2");
  assertEquals(a("a: |+\n  l1\n  l2\n\n"), "l1\nl2\n\n");
});

test("XML requires a root element", async () => {
  const { XML } = await import("runtime:serialization");
  // `""` parsed to `{}` and `"not xml at all"` came back as that same string,
  // so anything at all parsed "successfully" and validating input before
  // trusting it told you nothing.
  for (const src of ["", "   ", "not xml at all", "<?xml version=\"1.0\"?>"]) {
    assertThrows(() => XML.parse(src), "SyntaxError");
    assertEquals(XML.validate(src), false, `validate ${JSON.stringify(src)}`);
  }
  // A self-closing root is still a root.
  assertEquals(XML.validate("<r/>"), true);
  // An empty element is the empty string, which is the shape this parser has
  // always used for one — the point here is only that it *is* a root.
  assertEquals(JSON.stringify(XML.parse("<r/>")), '{"r":""}');
});

test("CSV parses as arrays by default and objects with header", async () => {
  const { CSV } = await import("runtime:serialization");

  // Default: array of field arrays, every field a string, no typing.
  assertEquals(JSON.stringify(CSV.parse("a,b\n1,2\n")), JSON.stringify([["a", "b"], ["1", "2"]]));

  // A quoted field keeps its delimiter and newline; doubled quotes unescape.
  assertEquals(JSON.stringify(CSV.parse('"a,b","c\nd"\n')), JSON.stringify([["a,b", "c\nd"]]));
  assertEquals(JSON.stringify(CSV.parse('"say ""hi"""\n')), JSON.stringify([['say "hi"']]));

  // CRLF and a bare CR both end records; a trailing newline adds no row.
  assertEquals(JSON.stringify(CSV.parse("a,b\r\n1,2\r\n")), JSON.stringify([["a", "b"], ["1", "2"]]));
  assertEquals(JSON.stringify(CSV.parse("a,b\r1,2")), JSON.stringify([["a", "b"], ["1", "2"]]));

  // header:true consumes the first row into keys.
  assertEquals(
    JSON.stringify(CSV.parse("name,age\nAlice,30\n", { header: true })),
    JSON.stringify([{ name: "Alice", age: "30" }]),
  );

  // Explicit names treat every row as data.
  assertEquals(
    JSON.stringify(CSV.parse("1,2\n", { header: ["x", "y"] })),
    JSON.stringify([{ x: "1", y: "2" }]),
  );

  // cast is off unless asked for.
  assertEquals(typeof CSV.parse("n\n1\n", { header: true })[0].n, "string");
  assertEquals(CSV.parse("n\n1\n", { header: true, cast: true })[0].n, 1);
  // …and never invents a value the source did not spell out.
  assertEquals(CSV.parse("n\ninf\n", { header: true, cast: true })[0].n, "inf");

  // Malformed input throws rather than returning a result envelope.
  assertThrows(() => CSV.parse('"a,b'), "SyntaxError");
  assertThrows(() => CSV.parse('"a"b\n'), "SyntaxError");
});

test("CSV validates, and validity follows the parse options", async () => {
  const { CSV } = await import("runtime:serialization");
  assertEquals(CSV.validate("a,b\n1,2\n"), true);
  assertEquals(
    JSON.stringify(CSV.validate("a,b\n1,2\n", { detailed: true })),
    JSON.stringify({ valid: true }),
  );
  // A ragged row is fine without a header but an error once one is declared —
  // which is why `validate` forwards the parse options.
  assertEquals(CSV.validate("a,b\n1\n"), true);
  assertEquals(CSV.validate("a,b\n1\n", { header: true }), false);
  const bad = CSV.validate("a,b\n1\n", { header: true, detailed: true });
  assert(bad.valid === false && typeof bad.error === "string", "detailed invalid shape");
});

test("CSV build quotes only when needed and round-trips", async () => {
  const { CSV } = await import("runtime:serialization");
  assertEquals(CSV.build([["a", "b,c", 'd"e', "f\ng"]]), 'a,"b,c","d""e","f\ng"\n');
  // Objects infer a header from the first row.
  assertEquals(CSV.build([{ name: "Alice", age: 30 }]), "name,age\nAlice,30\n");
  // Round-trip back through parse.
  const rows = CSV.parse('name,note\nAlice,"hi, there"\n', { header: true });
  assertEquals(JSON.stringify(CSV.parse(CSV.build(rows), { header: true })), JSON.stringify(rows));
});

test("CSV round-trips and refuses the shapes the reader would misread", async () => {
  const { CSV } = await import("runtime:serialization");

  // A field starting with the comment character is quoted, so the row survives
  // a round trip instead of being dropped as a comment line.
  const built = CSV.build([["#note", "val"]], { comment: "#" });
  assertEquals(built, '"#note",val\n');
  assertEquals(
    JSON.stringify(CSV.parse(built, { comment: "#" })),
    JSON.stringify([["#note", "val"]]),
  );

  // An absent (`undefined`) cell is an empty field, not an error.
  assertEquals(CSV.build([["a", undefined]]), "a,\n");

  // A decimal that overflows f64 stays the string it was, so `build` can still
  // write it.
  assertEquals(CSV.parse("1e500\n", { cast: true })[0][0], "1e500");

  // Named columns share the inferred header's duplicate rule.
  assertThrows(() => CSV.parse("1,2\n", { header: ["a", "a"] }), "SyntaxError");

  // A delimiter or quote that cannot be told apart is refused.
  assertThrows(() => CSV.parse("a,b\n", { delimiter: '"' }), "TypeError");
  assertThrows(() => CSV.parse("a,b\n", { quote: "\n" }), "TypeError");
});

test("CSV DecoderStream parses identically at any chunk boundary", async () => {
  const { CSV } = await import("runtime:serialization");

  const decode = async (chunks, options) => {
    const dec = new CSV.DecoderStream(options);
    const writer = dec.writable.getWriter();
    const reader = dec.readable.getReader();
    const out = [];
    const drain = (async () => {
      for (;;) { const { done, value } = await reader.read(); if (done) break; out.push(value); }
    })();
    for (const chunk of chunks) await writer.write(chunk);
    await writer.close();
    await drain;
    return out;
  };

  // A quoted newline, a CRLF and a doubled quote all cross the boundaries of
  // some split, so every split point must equal the one-shot parse.
  const text = 'name,note\r\nAlice,"hi, there"\r\nBob,"say ""hi"""\r\n';
  const whole = JSON.stringify(CSV.parse(text, { header: true }));
  for (let i = 1; i < text.length; i++) {
    const rows = await decode([text.slice(0, i), text.slice(i)], { header: true });
    assertEquals(JSON.stringify(rows), whole, `split at ${i}`);
  }

  // Byte chunks, with a multi-byte character split mid-sequence.
  const bytes = new TextEncoder().encode('name\n"café"\n');
  const rows = await decode([bytes.slice(0, 10), bytes.slice(10)]);
  assertEquals(JSON.stringify(rows), JSON.stringify([["name"], ["café"]]));

  // An unterminated quote is only knowable at the end, and still throws.
  await assertRejects(() => decode(['"a,b']), "SyntaxError");
});

test("CSV EncoderStream writes rows like build", async () => {
  const { CSV } = await import("runtime:serialization");

  const encode = async (rows, options) => {
    const enc = new CSV.EncoderStream(options);
    const writer = enc.writable.getWriter();
    const reader = enc.readable.getReader();
    let out = "";
    const drain = (async () => {
      for (;;) { const { done, value } = await reader.read(); if (done) break; out += value; }
    })();
    for (const row of rows) await writer.write(row);
    await writer.close();
    await drain;
    return out;
  };

  // Object rows: the first fixes the header, exactly as build infers it.
  const objects = [{ name: "Alice", age: 30 }, { name: "Bob", age: 41 }];
  assertEquals(await encode(objects), CSV.build(objects));
  assertEquals(await encode([["a", "b,c"]]), CSV.build([["a", "b,c"]]));

  // Mixing shapes is refused, exactly as build refuses a mixed array.
  await assertRejects(() => encode([["a"], { x: 1 }]), "TypeError");
});

test("runtime:serialization has a default export like every other runtime: module", async () => {
  // It was the only one of the nine without one, so
  // `import serialization from "runtime:serialization"` was a SyntaxError for
  // no reason a caller could see.
  const mod = await import("runtime:serialization");
  assert(mod.default !== undefined, "a default export must exist");
  assertEquals(
    Object.keys(mod.default).sort().join(","),
    "CBOR,CSV,JSONL,MessagePack,Protobuf,TOML,XML,YAML",
  );
  // The aggregate carries the real namespaces, not copies of their names.
  assertEquals(mod.default.YAML, mod.YAML);
  assertEquals(mod.default.XML, mod.XML);
  assertEquals(mod.default.CSV, mod.CSV);
  assertEquals(mod.default.CBOR, mod.CBOR);
  assertEquals(mod.default.Protobuf, mod.Protobuf);
  assertEquals(typeof mod.default.MessagePack.encode, "function");
});
