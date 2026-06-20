// Generate a large mock TOML document for benching
let tomlDoc = ``;
for (let i = 0; i < 5000; i++) {
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

let esrunParser = null;
try {
  const mod = await import('runtime:parsers');
  esrunParser = mod.TOMLParser.parse;
} catch (e) {}
const isEsrun = typeof esrunParser === "function";

let jsToml = null;

if (!isEsrun) {
  const mod = await import('@iarna/toml');
  jsToml = mod.default || mod;
}

function parseTOML() {
  if (isEsrun) {
    esrunParser(tomlDoc);
  } else if (jsToml) {
    jsToml.parse(tomlDoc);
  }
}

// Warmup
for (let i = 0; i < 5; i++) {
  parseTOML();
}

// Timed run
const iterations = 10;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseTOML();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
