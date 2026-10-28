// Generate a large JSONC document: records with comments and trailing commas,
// the shape of a big generated config.
const records = [];
for (let i = 0; i < 5000; i++) {
  records.push(`  // record ${i}
  {
    "id": "bk${i}", /* the catalog id */
    "author": "Gambardella, Matthew",
    "title": "XML Developer's Guide",
    "price": 44.95,
    "tags": ["computer", "xml",],
    "inStock": true,
  },`);
}
const jsoncDoc = `[\n${records.join("\n")}\n]\n`;

// Each runtime parses with the best facility it actually ships: esrun's native
// `runtime:serialization`, Bun's built-in `Bun.JSONC`, Deno's `@std/jsonc`, and
// Microsoft's jsonc-parser (what VS Code reads settings with) for Node, which
// ships no JSONC reader at all.
let parse = null;
try {
  const mod = await import('runtime:serialization');
  parse = (doc) => mod.JSONC.parse(doc);
} catch (e) {}
if (!parse && typeof Bun !== 'undefined') {
  parse = (doc) => Bun.JSONC.parse(doc);
}
if (!parse && typeof Deno !== 'undefined') {
  const { parse: denoParse } = await import('jsr:@std/jsonc@1.0.3');
  parse = (doc) => denoParse(doc);
}
if (!parse) {
  const { parse: nodeParse } = await import('jsonc-parser');
  parse = (doc) => nodeParse(doc, [], { allowTrailingComma: true });
}

// Every reader must agree on what it read, or the cell would compare less work.
const read = parse(jsoncDoc);
if (read.length !== 5000 || read[4999].id !== 'bk4999' || read[0].tags.length !== 2) {
  throw new Error('the document was not read correctly');
}

function parseJSONC() {
  parse(jsoncDoc);
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseJSONC();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseJSONC();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
