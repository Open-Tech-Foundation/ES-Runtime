// Protobuf decode (small payload). Same library — protobuf-es (@bufbuild/protobuf)
// — on every runtime, so this measures the runtime, not the library.
import { create, toBinary, fromBinary } from "@bufbuild/protobuf";
import { CatalogSchema } from "../gen/catalog_pb.js";

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

const protoBytes = toBinary(CatalogSchema, create(CatalogSchema, obj));
const parseProtobuf = () => fromBinary(CatalogSchema, protoBytes);

// Warmup
for (let i = 0; i < 5; i++) parseProtobuf();

// Timed run
const iterations = 1000;
const start = performance.now();
for (let i = 0; i < iterations; i++) parseProtobuf();
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
