// Generate a large mock YAML document for benching
let yamlDoc = `catalog:\n`;
for (let i = 0; i < 5000; i++) {
  yamlDoc += `  - id: bk${i}
    author: Gambardella, Matthew
    title: XML Developer's Guide
    genre: Computer
    price: 44.95
    publish_date: 2000-10-01
    description: An in-depth look at creating applications with XML.
`;
}

let esrunParser = null;
try {
  const mod = await import('runtime:parsers');
  esrunParser = mod.YAMLParser.parse;
} catch (e) {}
const isEsrun = typeof esrunParser === "function";
const isLlrt = typeof process !== 'undefined' && process.release?.name === 'llrt';

let jsYaml = null;

if (!isEsrun) {
  const mod = await import('js-yaml');
  jsYaml = mod.default || mod;
}

function parseYAML() {
  if (isEsrun) {
    esrunParser(yamlDoc);
  } else if (jsYaml) {
    jsYaml.load(yamlDoc);
  }
}

// Warmup
for (let i = 0; i < 5; i++) {
  parseYAML();
}

// Timed run
const iterations = 10;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseYAML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
