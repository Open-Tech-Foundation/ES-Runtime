// Generate a large mock XML document for benching
let xmlDoc = `<?xml version="1.0" encoding="UTF-8"?>\n<catalog>\n`;
for (let i = 0; i < 5000; i++) {
  xmlDoc += `
  <book id="bk${i}">
    <author>Gambardella, Matthew</author>
    <title>XML Developer's Guide</title>
    <genre>Computer</genre>
    <price>44.95</price>
    <publish_date>2000-10-01</publish_date>
    <description>An in-depth look at creating applications with XML.</description>
  </book>`;
}
xmlDoc += `\n</catalog>`;

// esrun's parser through its public API — guest code has no op table to call
// (D253), and the wrapper is part of what a program pays anyway.
let esrunParser = null;
try {
  const { XML } = await import("runtime:serialization");
  esrunParser = (xml) => XML.parse(xml);
} catch {
  // Not esrun.
}
const isEsrun = typeof esrunParser === "function";
const isLlrt = typeof process !== 'undefined' && process.release?.name === 'llrt';

let fxParser = null;
let llrtParse = null;

if (isLlrt) {
  const xmlModule = await import('llrt:xml');
  const parser = new xmlModule.XMLParser();
  llrtParse = (xml) => parser.parse(xml);
} else if (!isEsrun) {
  const { XMLParser: FXParser } = await import('fast-xml-parser');
  fxParser = new FXParser();
}

function parseXML() {
  if (isEsrun) {
    esrunParser(xmlDoc);
  } else if (isLlrt) {
    llrtParse(xmlDoc);
  } else {
    fxParser.parse(xmlDoc);
  }
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started at full speed; on the large
// documents one parse already does enough work to tier up, so the floor holds.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseXML();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseXML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
