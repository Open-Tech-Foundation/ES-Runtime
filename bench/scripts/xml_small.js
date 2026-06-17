// Generate a small mock XML document for benching
let xmlDoc = `<?xml version="1.0" encoding="UTF-8"?>\n<catalog>\n`;
for (let i = 0; i < 50; i++) {
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

let esrunParser = null;
if (typeof globalThis !== 'undefined' && globalThis.__ops && globalThis.__ops.xml_parse) {
  esrunParser = globalThis.__ops.xml_parse;
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

// Warmup
for (let i = 0; i < 5; i++) {
  parseXML();
}

// Timed run
const iterations = 500;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseXML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
