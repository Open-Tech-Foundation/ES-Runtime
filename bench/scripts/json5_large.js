// Generate a large JSON5 document: unquoted keys, single-quoted strings, hex,
// leading decimal points, comments and trailing commas.
const records = [];
for (let i = 0; i < 5000; i++) {
  records.push(`  // record ${i}
  {
    id: 'bk${i}',
    author: 'Gambardella, Matthew',
    title: "XML Developer's Guide",
    price: 44.95,
    discount: .15,
    flags: 0x${(i % 256).toString(16)},
    tags: ['computer', 'xml',],
    inStock: true,
  },`);
}
const json5Doc = `[\n${records.join("\n")}\n]\n`;

// Each runtime parses with the best facility it actually ships: esrun's native
// `runtime:serialization`, Bun's built-in `Bun.JSON5`, and the json5 reference
// package for Node and Deno, which ship no JSON5 reader at all.
let parse = null;
try {
  const mod = await import('runtime:serialization');
  parse = (doc) => mod.JSON5.parse(doc);
} catch (e) {}
if (!parse && typeof Bun !== 'undefined') {
  parse = (doc) => Bun.JSON5.parse(doc);
}
if (!parse) {
  const JSON5 = (await import(typeof Deno !== 'undefined' ? 'npm:json5@2.2.3' : 'json5')).default;
  parse = (doc) => JSON5.parse(doc);
}

// Every reader must agree on what it read, or the cell would compare less work.
const read = parse(json5Doc);
if (read.length !== 5000 || read[4999].id !== 'bk4999' || read[255].flags !== 255) {
  throw new Error('the document was not read correctly');
}

function parseJSON5() {
  parse(json5Doc);
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseJSON5();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseJSON5();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
