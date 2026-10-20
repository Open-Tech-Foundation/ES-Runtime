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
test("MessagePack spells a non-string map key the same with or without a bin", async () => {
  const { MessagePack, CBOR } = await import("runtime:serialization");
  // Each key alone (JSON-shaped) and beside a `bin` (value tree) must agree,
  // and agree with CBOR, which uses the same key rule.
  const keys = [
    [[0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff], "18446744073709551615"],
    [[0xcb, 0x3f, 0xf0, 0, 0, 0, 0, 0, 0], "1"],
    [[0xcb, 0x44, 0x4b, 0x1a, 0xe4, 0xd6, 0xe2, 0xef, 0x50], "1e+21"],
    [[0xc0], "null"],
  ];
  for (const [key, name] of keys) {
    const plain = MessagePack.decode(new Uint8Array([0x81, ...key, 0x01]));
    const withBin = MessagePack.decode(new Uint8Array([0x82, ...key, 0x01, 0xa1, 0x62, 0xc4, 0x00]));
    assertEquals(Object.keys(plain)[0], name, `plain ${name}`);
    assertEquals(Object.keys(withBin).find((k) => k !== "b"), name, `with bin ${name}`);
  }
  // CBOR: uint64 max and the float 1.0 as keys.
  const cbor = CBOR.decode(new Uint8Array([0xa2, 0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01, 0xf9, 0x3c, 0x00, 0x02]));
  assertEquals(Object.keys(cbor).sort().join(","), "1,18446744073709551615");
});

test("MessagePack decode and validate refuse trailing bytes, bad UTF-8 and deep nesting", async () => {
  const { MessagePack } = await import("runtime:serialization");
  const bad = [
    [0x01, 0x02], // a byte after the top-level value
    [0xa1, 0xff], // a str that is not UTF-8 (was decoded as [255])
    [...new Array(300).fill(0x91), 0x01], // past the 256-level bound
  ];
  for (const bytes of bad) {
    const input = new Uint8Array(bytes);
    assertThrows(() => MessagePack.decode(input), "SyntaxError");
    assertEquals(MessagePack.validate(input), false);
  }
});

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

test("CBOR spells large numeric map keys as the guest would", async () => {
  const { CBOR } = await import("runtime:serialization");
  // A float64 key at 1e20 is written out in full, and at 1e21 String(n)
  // switches to exponential notation. A saturating cast would spell 1e20 as
  // i64::MAX and collapse it with every other large key.
  const doc = CBOR.decode(
    new Uint8Array([
      0xa2, 0xfb, 0x44, 0x15, 0xaf, 0x1d, 0x78, 0xb5, 0x8c, 0x40, 0x01,
      0xfb, 0x44, 0x4b, 0x1a, 0xe4, 0xd6, 0xe2, 0xef, 0x50, 0x02,
    ]),
  );
  assertEquals(Object.keys(doc).join(), "100000000000000000000,1e+21");
});

test("CBOR coerces non-string map keys like a property access", async () => {
  const { CBOR } = await import("runtime:serialization");
  // An array key [1, 2] spells "1,2"; a nested map key is "[object Object]".
  const arr = CBOR.decode(new Uint8Array([0xa1, 0x82, 0x01, 0x02, 0x03]));
  assertEquals(Object.keys(arr).join(), "1,2");
  const obj = CBOR.decode(new Uint8Array([0xa1, 0xa1, 0x61, 0x61, 0x01, 0x03]));
  assertEquals(Object.keys(obj).join(), "[object Object]");
});

test("CBOR keeps a __proto__ key as an own property", async () => {
  const { CBOR } = await import("runtime:serialization");
  // The map {__proto__: 7}. A plain `set` would run the inherited prototype
  // setter — losing the key and making 7 the prototype — so the field must be
  // defined as an own data property, exactly as `JSON.parse` does.
  const obj = CBOR.decode(
    new Uint8Array([
      0xa1, 0x69, 0x5f, 0x5f, 0x70, 0x72, 0x6f, 0x74, 0x6f, 0x5f, 0x5f, 0x07,
    ]),
  );
  assertEquals(Object.prototype.hasOwnProperty.call(obj, "__proto__"), true);
  assertEquals(obj.__proto__, 7);
  assertEquals(Object.getPrototypeOf(obj), Object.prototype);
});

test("CBOR and MessagePack encode an own __proto__ key", async () => {
  const { CBOR, MessagePack } = await import("runtime:serialization");
  // A parsed document (or a Map) can carry an own `__proto__` field. Building
  // the host object with a plain `{}` assignment ran the inherited prototype
  // setter, so the field was dropped; a null-prototype object keeps it.
  const parsed = JSON.parse('{"__proto__": 1, "x": 2}');
  assertEquals(Object.prototype.hasOwnProperty.call(parsed, "__proto__"), true);
  for (const codec of [CBOR, MessagePack]) {
    const back = codec.decode(codec.encode(parsed));
    assertEquals(Object.prototype.hasOwnProperty.call(back, "__proto__"), true);
    assertEquals(back.__proto__, 1);
    assertEquals(back.x, 2);
  }
  const viaMap = CBOR.decode(CBOR.encode(new Map([["__proto__", 3]])));
  assertEquals(Object.prototype.hasOwnProperty.call(viaMap, "__proto__"), true);
  assertEquals(viaMap.__proto__, 3);
});

test("CBOR decodes negative integers just past 2^53 exactly", async () => {
  const { CBOR } = await import("runtime:serialization");
  // Major type 1, argument 2^53 + 1: the CBOR integer is -9007199254740994,
  // past the safe range, so it decodes to a bigint — exactly. Subtracting in
  // floating point used to round the argument first and produce -9007199254740992.
  const bytes = [0x3b, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01];
  assertEquals(CBOR.decode(new Uint8Array(bytes)), -9007199254740994n);
  assertEquals([...CBOR.encode(-9007199254740994n)].join(), bytes.join());
});

test("CBOR uses BigInt past the safe integer range", async () => {
  const { CBOR } = await import("runtime:serialization");
  // Inside the safe range an integer is a number; past it a bigint, so no CBOR
  // integer is ever silently rounded (unlike MessagePack, which has no BigInt).
  assertEquals(typeof CBOR.decode(CBOR.encode(9007199254740991)), "number");
  assertEquals(typeof CBOR.decode(CBOR.encode(9007199254740993n)), "bigint");
  assertEquals(CBOR.decode(CBOR.encode(9007199254740993n)), 9007199254740993n);
  // The decoded value is exact, and a `number` cannot spell it.
  assertEquals(
    CBOR.decode(new Uint8Array([0x1b, 0x00, 0x20, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01])),
    9007199254740993n,
  );
  // The widest integers the major types carry, both ways.
  assertEquals(CBOR.decode(CBOR.encode(18446744073709551615n)), 18446744073709551615n);
  assertEquals(CBOR.decode(CBOR.encode(-18446744073709551616n)), -18446744073709551616n);
  // A BigInt inside the safe range has no BigInt form on the wire: it encodes
  // as an integer and decodes back as a number.
  assertEquals(typeof CBOR.decode(CBOR.encode(5n)), "number");
  // Beyond that the format needs a bignum tag, which is refused.
  assertThrows(() => CBOR.encode(18446744073709551616n), "TypeError");
  // A BigInt too large even for the host bridge is refused, not truncated.
  assertThrows(() => CBOR.encode(2n ** 200n), "RangeError");
});

test("CBOR keeps negative integers in integer form down to -2^64", async () => {
  const { CBOR } = await import("runtime:serialization");
  // Major type 1 reaches -2^64 (0x3b then eight 0xff); the encoder used to
  // stop at i64::MIN and write a float. Decoding yields a bigint at this
  // magnitude, which is where the format's own floor is.
  assertEquals([...CBOR.encode(-18446744073709551616)].join(), "59,255,255,255,255,255,255,255,255");
  assertEquals(CBOR.decode(CBOR.encode(-18446744073709551616)), -18446744073709551616n);
  // One `f64` lower has no integer form, so it stays a float (0xfb).
  assertEquals(CBOR.encode(-(18446744073709551616 + 4096))[0], 0xfb);
});

test("CBOR refuses a two-byte simple value below 32 and unwraps boxed primitives", async () => {
  const { CBOR, MessagePack } = await import("runtime:serialization");

  // RFC 8949 §3.3: 0xf8 0x14 is not well-formed — `false` has one spelling.
  assertThrows(() => CBOR.decode(new Uint8Array([0xf8, 0x14])), "SyntaxError");
  assertEquals(CBOR.validate(new Uint8Array([0xf8, 0x14])), false);

  // A boxed primitive is the value it wraps, not `{}` or its indexed chars.
  const boxed = [new String("x"), new Number(1), new Boolean(false), Object(5n)];
  assertEquals(JSON.stringify(CBOR.decode(CBOR.encode(boxed))), '["x",1,false,5]');
  assertEquals(
    JSON.stringify(MessagePack.decode(MessagePack.encode(boxed.slice(0, 3)))),
    '["x",1,false]',
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

test("XML keeps references, CDATA and empty elements, and refuses what is not well formed", async () => {
  const { XML } = await import("runtime:serialization");
  // References used to vanish (`t&lt;` parsed to `t`) and CDATA was dropped.
  assertEquals(
    JSON.stringify(XML.parse('<a x="&amp;&#65;">t&lt;<![CDATA[ & ]]></a>')),
    '{"a":{"@x":"&A","$text":"t< & "}}',
  );
  // `<a></a>` and `<a/>` are the same element.
  assertEquals(JSON.stringify(XML.parse("<r><a></a><b/></r>")), '{"r":{"a":"","b":""}}');
  for (const src of ["<a/><b/>", "<a/>junk", "<a>&foo;</a>", '<a x="1" x="2"/>', "<1a/>"]) {
    assertThrows(() => XML.parse(src), "SyntaxError");
    assertEquals(XML.validate(src), false, `validate ${src}`);
  }
});

test("XML build writes any number and refuses what XML cannot spell", async () => {
  const { XML } = await import("runtime:serialization");
  // A non-integer number threw (the JSON pivot's private number map).
  assertEquals(XML.build({ r: { a: 0.1, b: 1e21 } }), "<r><a>0.1</a><b>1e+21</b></r>");
  const value = { r: { "@a": "1&2", x: ["t<", " u "] } };
  assertEquals(JSON.stringify(XML.parse(XML.build(value))),
    '{"r":{"@a":"1&2","x":[{"$text":"t<"},{"$text":" u "}]}}');
  assertThrows(() => XML.build({ r: { n: NaN } }), "TypeError");
  assertThrows(() => XML.build({ r: { b: new Uint8Array([1]) } }), "TypeError");
  assertThrows(() => XML.build({ items: [1, 2] }), "TypeError");
});

test("XML DecoderStream errors on a malformed or truncated document", async () => {
  const { XML } = await import("runtime:serialization");
  const decode = async (chunks) => {
    const dec = new XML.DecoderStream();
    const writer = dec.writable.getWriter();
    const reader = dec.readable.getReader();
    const out = [];
    const drain = (async () => {
      for (;;) { const { done, value } = await reader.read(); if (done) break; out.push(value); }
    })();
    try {
      for (const chunk of chunks) await writer.write(chunk);
      await writer.close();
    } finally {
      await drain;
    }
    return out;
  };
  // A character split across byte chunks decodes once, not as two U+FFFDs.
  const bytes = new TextEncoder().encode("<r><i>café</i></r>");
  assertEquals(JSON.stringify(await decode([bytes.slice(0, 10), bytes.slice(10)])), '[{"i":{"$text":"café"}}]');
  // A malformed record was skipped and a truncated document ended quietly.
  await assertRejects(() => decode(["<r><i>a</j><i>b</i></r>"]), "SyntaxError");
  await assertRejects(() => decode(["<r><i>a</i><i>b"]), "SyntaxError");
});

test("YAML build spells numbers and BigInts exactly and refuses what it would drop", async () => {
  const { YAML } = await import("runtime:serialization");
  assertEquals(YAML.build({ a: 2 ** 63, b: 1e21 }), "a: 9223372036854776000\nb: 1e+21\n");
  assertEquals(YAML.build({ big: 12345678901234567890n }), "big: 12345678901234567890\n");
  // These became `null` or `{}`.
  assertEquals(YAML.build({ d: new Date(0) }), "d: 1970-01-01T00:00:00.000Z\n");
  assertEquals(YAML.build({ m: new Map([["k", 1]]) }), "m:\n  k: 1\n");
  assertThrows(() => YAML.build({ b: new Uint8Array([1]) }), "TypeError");
  assertThrows(() => YAML.build({ f() {} }), "TypeError");
});

test("TOML keeps key order and integers exact, and carries nan, inf and BigInt", async () => {
  const { TOML } = await import("runtime:serialization");
  assertEquals(Object.keys(TOML.parse("zeta = 1\nalpha = 2")).join(","), "zeta,alpha");
  assertEquals(TOML.build({ zeta: 1, alpha: 2 }), "zeta = 1\nalpha = 2\n");
  // An integer past 2^53 is a BigInt rather than a rounded number.
  assertEquals(TOML.parse("n = 9007199254740993").n, 9007199254740993n);
  assertEquals(typeof TOML.parse("n = 42").n, "number");
  assertEquals(TOML.build({ a: NaN, b: -Infinity, c: 5n }), "a = nan\nb = -inf\nc = 5\n");
  // A large number stays a number on the way back, not i64::MAX.
  assertEquals(TOML.parse(TOML.build({ n: 2 ** 63 })).n, 2 ** 63);
  // A Date was written as an empty table.
  assertEquals(TOML.build({ d: new Date(0) }), 'd = "1970-01-01T00:00:00.000Z"\n');
  assertThrows(() => TOML.build({ b: new Uint8Array([1]) }), "TypeError");
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

test("CSV options keep a round trip lossless", async () => {
  const { CSV } = await import("runtime:serialization");

  // A comment byte equal to the delimiter or quote makes an ordinary record
  // read as a comment, so the combination is refused.
  assertThrows(() => CSV.build([["", "x"]], { delimiter: "#", comment: "#" }), "TypeError");
  assertThrows(() => CSV.parse('"a"\n', { comment: '"' }), "TypeError");

  // `trim` skips blanks around a quoted field instead of splitting it.
  assertEquals(
    JSON.stringify(CSV.parse('a, "b, c" \n', { trim: true })),
    JSON.stringify([["a", "b, c"]]),
  );

  // `cast` keeps an integer it cannot hold exactly as the string it was.
  assertEquals(
    JSON.stringify(CSV.parse("12345678901234567890,42\n", { cast: true })),
    JSON.stringify([["12345678901234567890", 42]]),
  );

  // `build` spells numbers as `String(n)` does, and a BigInt exactly.
  assertEquals(CSV.build([[1e300, 1e-7, 0.1, -0]]), "1e+300,1e-7,0.1,0\n");
  assertEquals(CSV.build([[12345678901234567890n]]), "12345678901234567890\n");
});

test("CSV DecoderStream honours bom for byte input and releases on cancel", async () => {
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

  // The host's `bom` option decides for bytes exactly as it does for `parse`;
  // TextDecoder no longer strips the BOM first.
  const bytes = new Uint8Array([0xef, 0xbb, 0xbf, 0x61, 0x2c, 0x62, 0x0a]);
  assertEquals(
    JSON.stringify(await decode([bytes], { bom: false })),
    JSON.stringify(CSV.parse("\ufeffa,b\n", { bom: false })),
  );
  assertEquals(
    JSON.stringify(await decode([bytes.slice(0, 2), bytes.slice(2)])),
    JSON.stringify([["a", "b"]]),
  );

  // Canceling mid-field (an open quote) releases the stream quietly rather
  // than surfacing the unterminated quote nobody is reading any more.
  const dec = new CSV.DecoderStream();
  const writer = dec.writable.getWriter();
  writer.write('"open').catch(() => {});
  await dec.readable.cancel("done");
  await writer.closed.catch(() => {});
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
