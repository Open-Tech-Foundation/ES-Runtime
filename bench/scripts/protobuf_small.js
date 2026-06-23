// Protobuf decode (small payload). Real-world positioning: esrun decodes with its
// native runtime:serialization Protobuf (pure-JS, reflective); Node/Bun/Deno decode
// with protobuf-es (@bufbuild/protobuf) — each runtime uses the lib it would ship.
// The wire bytes are identical, so this compares each runtime's actual protobuf path.
const PROTO = `
syntax = "proto3";
package test;
message Book {
  string id = 1; string author = 2; string title = 3; string genre = 4;
  double price = 5; string publish_date = 6; string description = 7;
}
message Catalog { repeated Book catalog = 1; }
`;

let schema = null;
try {
  const { Protobuf } = await import("runtime:serialization");
  schema = new Protobuf.Schema(PROTO);
} catch (e) {}
const isEsrun = schema !== null;

let create, toBinary, fromBinary, CatalogSchema;
if (!isEsrun) {
  ({ create, toBinary, fromBinary } = await import("@bufbuild/protobuf"));
  ({ CatalogSchema } = await import("../gen/catalog_pb.js"));
}

const obj = { catalog: [] };
for (let i = 0; i < 50; i++) {
  obj.catalog.push({
    id: `bk${i}`,
    author: "Gambardella, Matthew",
    title: "XML Developer's Guide",
    genre: "Computer",
    price: 44.95,
    publishDate: "2000-10-01",
    description: "An in-depth look at creating applications with XML.",
  });
}

let parseProtobuf;
if (isEsrun) {
  const protoBytes = schema.encode("test.Catalog", obj);
  parseProtobuf = () => schema.decode("test.Catalog", protoBytes);
} else {
  const protoBytes = toBinary(CatalogSchema, create(CatalogSchema, obj));
  parseProtobuf = () => fromBinary(CatalogSchema, protoBytes);
}

// Warmup
for (let i = 0; i < 5; i++) parseProtobuf();

// Timed run
const iterations = 1000;
const start = performance.now();
for (let i = 0; i < iterations; i++) parseProtobuf();
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
