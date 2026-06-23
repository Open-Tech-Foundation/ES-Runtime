// Differential parity for our reflective protobuf lib (crates/runtime/js) vs the
// reference protobuf-es, on the catalog + all schemas. Asserts byte-for-byte
// encode equality and cross-implementation decode. Run: bun protobuf-parity.mjs
import { readFileSync } from "node:fs";
import { create, toBinary, fromBinary } from "@bufbuild/protobuf";
import { CatalogSchema } from "./gen/catalog_pb.js";
import { AllSchema } from "./gen/all_pb.js";
import { Schema } from "../crates/runtime/js/serialization/protobuf/schema.ts";

const eq = (a, b) => a.length === b.length && [...a].every((x, i) => x === b[i]);
let fails = 0;
const check = (name, cond) => { console.log((cond ? "ok  - " : "FAIL- ") + name); if (!cond) fails++; };

// --- catalog: strings, double, repeated message ---
{
  const ours = new Schema(readFileSync(new URL("./proto/catalog.proto", import.meta.url), "utf8"));
  const obj = { catalog: [] };
  for (let i = 0; i < 1000; i++) {
    obj.catalog.push({ id: `bk${i}`, author: "A", title: "T", genre: "G", price: 44.95, publishDate: "2000-10-01", description: "D" });
  }
  const es = toBinary(CatalogSchema, create(CatalogSchema, obj));
  const our = ours.build("test.Catalog", obj);
  check("catalog: byte-equal encode", eq(es, our));
  check("catalog: our decode of es bytes", ours.parse("test.Catalog", es).catalog[0].title === "T");
  check("catalog: es decode of our bytes", fromBinary(CatalogSchema, our).catalog[999].id === "bk999");
}

// --- all: enums, 64-bit BigInt, sint/fixed, packed, repeated message, map ---
{
  const ours = new Schema(readFileSync(new URL("./proto/all.proto", import.meta.url), "utf8"));
  const esInput = {
    i32: -7, i64: 9007199254740993n, u32: 7, u64: 18446744073709551615n,
    s32: -123, s64: -9007199254740993n, f32: 4294967295, f64: 18446744073709551615n,
    sf32: -42, sf64: -99n, fl: 1.5, db: 44.95, b: true, s: "héllo 𐍈",
    by: new Uint8Array([1, 2, 3, 255]), c: 2, inner: { v: "x", n: 9 },
    nums: [1, 2, 300, -4], items: [{ v: "a" }, { n: 5 }], counts: { x: 1, y: 2 },
  };
  const es = toBinary(AllSchema, create(AllSchema, esInput));
  const our = ours.build("t.All", { ...esInput, c: "BLUE" });
  check("all: byte-equal encode", eq(es, our));
  const od = ours.parse("t.All", es);
  check("all: our decode 64-bit + enum name", od.i64 === 9007199254740993n && od.c === "BLUE");
  check("all: es decode of our bytes", fromBinary(AllSchema, our).c === 2);
}

console.log(fails ? `\n${fails} PARITY FAILURE(S)` : "\nALL PARITY OK");
process.exit(fails ? 1 : 0);
