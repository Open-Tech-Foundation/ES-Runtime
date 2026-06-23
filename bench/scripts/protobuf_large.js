let esrunParser = null;
let esrunBuilder = null;
let esrunSchema = null;
try {
  const mod = await import('runtime:parsers');
  const schemaDef = `
    syntax = "proto3";
    package test;
    message Book {
      string id = 1;
      string author = 2;
      string title = 3;
      string genre = 4;
      double price = 5;
      string publish_date = 6;
      string description = 7;
    }
    message Catalog {
      repeated Book catalog = 1;
    }
  `;
  esrunSchema = new mod.Protobuf.Schema(schemaDef);
  esrunParser = (bytes) => esrunSchema.parse("test.Catalog", bytes);
  esrunBuilder = (obj) => esrunSchema.build("test.Catalog", obj);
} catch (e) {}
const isEsrun = typeof esrunParser === "function";

let protobufjs = null;
let root = null;
let Catalog = null;
if (!isEsrun) {
  const mod = await import('protobufjs');
  protobufjs = mod.default || mod;
  root = protobufjs.parse(`
    syntax = "proto3";
    package test;
    message Book {
      string id = 1;
      string author = 2;
      string title = 3;
      string genre = 4;
      double price = 5;
      string publish_date = 6;
      string description = 7;
    }
    message Catalog {
      repeated Book catalog = 1;
    }
  `).root;
  Catalog = root.lookupType("test.Catalog");
}

// Generate a large mock Object for benching
let obj = { catalog: [] };
for (let i = 0; i < 50000; i++) {
  obj.catalog.push({
    id: `bk${i}`,
    author: "Gambardella, Matthew",
    title: "XML Developer's Guide",
    genre: "Computer",
    price: 44.95,
    publish_date: "2000-10-01",
    description: "An in-depth look at creating applications with XML."
  });
}

let protoBytes;
if (isEsrun) {
    protoBytes = esrunBuilder(obj);
} else {
    protoBytes = Catalog.encode(Catalog.create(obj)).finish();
}

function parseProtobuf() {
  if (isEsrun) {
    esrunParser(protoBytes);
  } else if (Catalog) {
    Catalog.decode(protoBytes);
  }
}

// Warmup
for (let i = 0; i < 5; i++) {
  parseProtobuf();
}

// Timed run
const iterations = 50;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseProtobuf();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
