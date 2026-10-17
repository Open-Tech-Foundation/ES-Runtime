// Generate a large mock CSV document for benching
const rows = ["id,author,title,genre,price,publish_date,description"];
for (let i = 0; i < 5000; i++) {
  rows.push(
    `bk${i},"Gambardella, Matthew","XML Developer's Guide",Computer,44.95,2000-10-01,"An in-depth look at creating applications with XML."`,
  );
}
const csvDoc = rows.join("\n") + "\n";

// Each runtime parses with the best facility it actually ships: esrun's native
// `runtime:serialization`, Deno's `@std/csv`, and csv-parse for Node and Bun,
// which ship no CSV reader at all. Holding a runtime to a library it does not
// need would understate it.
let parse = null;
try {
  const mod = await import('runtime:serialization');
  parse = (doc) => mod.CSV.parse(doc);
} catch (e) {}
if (!parse && typeof Deno !== 'undefined') {
  const { parse: denoParse } = await import('jsr:@std/csv@1.0.6');
  parse = (doc) => denoParse(doc);
}
if (!parse) {
  const { parse: nodeParse } = await import('csv-parse/sync');
  parse = (doc) => nodeParse(doc);
}

function parseCSV() {
  parse(csvDoc);
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started full speed; on the large
// documents one parse already does enough work to tier up, so the floor holds.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseCSV();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseCSV();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
