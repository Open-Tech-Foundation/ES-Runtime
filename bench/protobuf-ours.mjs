// Decode throughput: our reflective Protobuf (runtime:serialization) vs the
// reference protobuf-es, on identical bytes. Runs in esrun (has runtime:
// serialization); protobuf-es also runs for an in-engine baseline.
import { create, toBinary, fromBinary } from "@bufbuild/protobuf";
import { CatalogSchema } from "./gen/catalog_pb.js";

const { Protobuf } = await import("runtime:serialization");
const proto = `
  syntax = "proto3";
  package test;
  message Book {
    string id = 1; string author = 2; string title = 3; string genre = 4;
    double price = 5; string publish_date = 6; string description = 7;
  }
  message Catalog { repeated Book catalog = 1; }
`;
const ours = new Protobuf.Schema(proto);

const obj = { catalog: [] };
for (let i = 0; i < 50000; i++) {
  obj.catalog.push({
    id: `bk${i}`, author: "Gambardella, Matthew", title: "XML Developer's Guide",
    genre: "Computer", price: 44.95, publishDate: "2000-10-01",
    description: "An in-depth look at creating applications with XML.",
  });
}
const bytes = toBinary(CatalogSchema, create(CatalogSchema, obj));
console.log(`payload: ${bytes.length} bytes, 50000 books`);

function bench(name, fn, iters = 30) {
  for (let i = 0; i < 5; i++) fn();
  const t0 = performance.now();
  for (let i = 0; i < iters; i++) fn();
  const t1 = performance.now();
  console.log(`${name.padEnd(28)}: ${((t1 - t0) / iters).toFixed(1)} ms/op`);
}

// correctness sanity
const a = ours.parse("test.Catalog", bytes);
console.log(`ours decoded ${a.catalog.length} books, first.price=${a.catalog[0].price}`);

const esMsg = create(CatalogSchema, obj);
bench("protobuf-es decode", () => fromBinary(CatalogSchema, bytes));
bench("ours decode (parse)", () => ours.parse("test.Catalog", bytes));
bench("protobuf-es encode", () => toBinary(CatalogSchema, esMsg));
bench("ours encode (build)", () => ours.build("test.Catalog", obj));
