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
