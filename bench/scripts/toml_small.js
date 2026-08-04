// Generate a small mock TOML document for benching
let tomlDoc = ``;
for (let i = 0; i < 50; i++) {
  tomlDoc += `
[[catalog]]
id = "bk${i}"
author = "Gambardella, Matthew"
title = "XML Developer's Guide"
genre = "Computer"
price = 44.95
publish_date = "2000-10-01"
description = "An in-depth look at creating applications with XML."
`;
}

// Each runtime parses with the best facility it actually ships: esrun's native
// `runtime:serialization`, Bun's native `Bun.TOML`, and @iarna/toml for Node and
// Deno, which have no built-in TOML parser. Holding Bun to a JS library it
// does not need would understate it by roughly 2x.
let parse = null;
try {
  const mod = await import('runtime:serialization');
  parse = (doc) => mod.TOML.parse(doc);
} catch (e) {}
if (!parse && typeof Bun !== 'undefined' && Bun.TOML?.parse) {
  parse = (doc) => Bun.TOML.parse(doc);
}
if (!parse) {
  const mod = await import('@iarna/toml');
  const jsToml = mod.default || mod;
  parse = (doc) => jsToml.parse(doc);
}

function parseTOML() {
  parse(tomlDoc);
}

// Timed run
const iterations = 500;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started at full speed; on the large
// documents one parse already does enough work to tier up, so the floor holds.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseTOML();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseTOML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
