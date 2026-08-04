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

// Each runtime parses with the best facility it actually ships: esrun's native
// `runtime:serialization`, Bun's native `Bun.YAML`, and js-yaml for Node and
// Deno, which have no built-in YAML parser. Holding Bun to a JS library it
// does not need would understate it by roughly 2x.
let parse = null;
try {
  const mod = await import('runtime:serialization');
  parse = (doc) => mod.YAML.parse(doc);
} catch (e) {}
if (!parse && typeof Bun !== 'undefined' && Bun.YAML?.parse) {
  parse = (doc) => Bun.YAML.parse(doc);
}
if (!parse) {
  const mod = await import('js-yaml');
  const jsYaml = mod.default || mod;
  parse = (doc) => jsYaml.load(doc);
}

function parseYAML() {
  parse(yamlDoc);
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started at full speed; on the large
// documents one parse already does enough work to tier up, so the floor holds.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseYAML();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseYAML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
