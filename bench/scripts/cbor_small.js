// Generate a small mock catalogue (the same shape as cbor_large).
const catalog = [];
for (let i = 0; i < 50; i++) {
  catalog.push({
    id: `bk${i}`,
    author: "Gambardella, Matthew",
    title: "XML Developer's Guide",
    genre: "Computer",
    price: 44.95,
    publish_date: "2000-10-01",
    description: "An in-depth look at creating applications with XML.",
  });
}
const obj = { catalog };

// Each runtime uses the CBOR facility it actually ships: esrun's native
// `runtime:serialization`, Deno's `@std/cbor`, and cbor-x for Node and Bun,
// which ship none. cbor-x's record extension is off (`useRecords: false`) so it
// writes plain CBOR maps, the format the other two write, rather than a private
// structure tag — otherwise the row would compare a packed record against CBOR.
let encode = null;
let decode = null;
try {
  const mod = await import('runtime:serialization');
  encode = (v) => mod.CBOR.encode(v);
  decode = (b) => mod.CBOR.decode(b);
} catch (e) {}
if (!encode && typeof Deno !== 'undefined') {
  const { encodeCbor, decodeCbor } = await import('jsr:@std/cbor@0.1.10');
  encode = (v) => encodeCbor(v);
  decode = (b) => decodeCbor(b);
}
if (!encode) {
  const { Encoder } = await import('cbor-x');
  const enc = new Encoder({ useRecords: false });
  encode = (v) => enc.encode(v);
  decode = (b) => enc.decode(b);
}

const cborBytes = encode(obj);

function parseCbor() {
  decode(cborBytes);
}

// Timed run
const iterations = 1000;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started at full speed; on the small
// document a parse is cheap, so the floor does the tiering up.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseCbor();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseCbor();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
