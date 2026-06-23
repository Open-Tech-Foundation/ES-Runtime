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
