let esrunParser = null;
let esrunBuilder = null;
try {
  const mod = await import('runtime:serialization');
  esrunParser = mod.MessagePack.decode;
  esrunBuilder = mod.MessagePack.encode;
} catch (e) {}
const isEsrun = typeof esrunParser === "function";

let msgpackr = null;
if (!isEsrun) {
  const mod = await import('msgpackr');
  msgpackr = mod;
}

// Generate a large mock Object for benching
let obj = { catalog: [] };
for (let i = 0; i < 5000; i++) {
  obj.catalog.push({
    id: `bk${i}`,
    author: "Gambardella, Matthew",
    title: "XML Developer's Guide",
    genre: "Computer",
    price: 44.95,
    publish_date: "2000-10-01",
    description: "An in-depth look at creating applications with XML."
  });
}

let msgpackBytes;
if (isEsrun) {
    msgpackBytes = esrunBuilder(obj);
} else {
    msgpackBytes = msgpackr.pack(obj);
}

function parseMsgpack() {
  if (isEsrun) {
    esrunParser(msgpackBytes);
  } else if (msgpackr) {
    msgpackr.unpack(msgpackBytes);
  }
}

// Timed run
const iterations = 10;

// Untimed warmup: a tenth of the timed run (the ratio the engine workloads use),
// never fewer than 5. A flat handful left the JIT-backed libraries measured
// part-way up the tiers while native parsers started at full speed; on the large
// documents one parse already does enough work to tier up, so the floor holds.
for (let i = 0; i < Math.max(iterations / 10, 5); i++) {
  parseMsgpack();
}
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseMsgpack();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
