test("protobuf schema parse/build round-trip", async () => {
const { Protobuf } = await import('runtime:parsers');

const schema = new Protobuf.Schema(`
    syntax = "proto3";
    package test;
    message Person {
        string name = 1;
        int32 id = 2;
        string email = 3;
    }
`);

const p = { name: "John Doe", id: 1234, email: "jdoe@example.com" };

const bytes = schema.build("test.Person", p);
if (!(bytes instanceof Uint8Array)) throw new Error("Expected Uint8Array");
if (bytes.length === 0) throw new Error("Expected non-empty bytes");

const decoded = schema.parse("test.Person", bytes);
if (decoded.name !== p.name) throw new Error(`Name mismatch: ${decoded.name}`);
if (decoded.id !== p.id) throw new Error(`ID mismatch: ${decoded.id}`);
if (decoded.email !== p.email) throw new Error(`Email mismatch: ${decoded.email}`);

// An unknown message name must throw rather than silently succeed.
let threw = false;
try { schema.build("test.Missing", p); } catch (e) { threw = true; }
if (!threw) throw new Error("Expected build with unknown message to throw");

// A malformed schema must throw a SyntaxError, not silently compile.
let schemaThrew = false;
try { new Protobuf.Schema('syntax = "proto3"; message {'); } catch (e) { schemaThrew = true; }
if (!schemaThrew) throw new Error("Expected malformed schema to throw");

// free() releases the schema and is idempotent.
schema.free();
if (schema.id !== null) throw new Error("Expected id to be null after free()");
schema.free();
});

test("protobuf advanced types follow the proto3 JSON mapping", async () => {
const { Protobuf } = await import('runtime:parsers');

using schema = new Protobuf.Schema(`
    syntax = "proto3";
    package x;
    enum Color { RED = 0; GREEN = 1; }
    message Inner { string v = 1; }
    message M {
        int64 big = 1;
        repeated int32 nums = 2;
        Color c = 3;
        Inner inner = 4;
        bytes data = 5;
        map<string, int32> counts = 6;
    }
`);

// 64-bit ints round-trip exactly as strings (no f64 precision loss); enums as
// names; repeated/nested/map carry through; bytes use base64 strings.
const b64 = btoa(String.fromCharCode(1, 2, 3));
const obj = {
    big: "9007199254740993",
    nums: [1, 2, 3],
    c: "GREEN",
    inner: { v: "hi" },
    data: b64,
    counts: { a: 1 },
};
const decoded = schema.parse("x.M", schema.build("x.M", obj));
if (decoded.big !== "9007199254740993") throw new Error("int64 should be an exact string: " + decoded.big);
if (typeof decoded.big !== "string") throw new Error("int64 must be a string");
if (JSON.stringify(decoded.nums) !== "[1,2,3]") throw new Error("repeated mismatch: " + JSON.stringify(decoded.nums));
if (decoded.c !== "GREEN") throw new Error("enum should be its name: " + decoded.c);
if (decoded.inner.v !== "hi") throw new Error("nested message mismatch");
if (decoded.data !== b64) throw new Error("bytes (base64) mismatch: " + decoded.data);
if (decoded.counts.a !== 1) throw new Error("map mismatch");

// Proto3 omits fields left at their default value.
const defaults = schema.parse("x.M", schema.build("x.M", {}));
if (Object.keys(defaults).length !== 0) throw new Error("expected no default fields, got " + JSON.stringify(defaults));

// Decoding a payload against an unknown message name throws.
assertThrows(() => schema.parse("x.Nope", new Uint8Array([0])));
});
