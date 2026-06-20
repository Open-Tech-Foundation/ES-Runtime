let esrunParser = null;
let esrunBuilder = null;
try {
  const mod = await import('runtime:parsers');
  esrunParser = mod.MessagePack.decode;
  esrunBuilder = mod.MessagePack.encode;
} catch (e) {}
const isEsrun = typeof esrunParser === "function";

let msgpackr = null;
if (!isEsrun) {
  const mod = await import('msgpackr');
  msgpackr = mod;
}

// Generate a small mock Object for benching
let obj = { catalog: [] };
for (let i = 0; i < 50; i++) {
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

// Warmup
for (let i = 0; i < 5; i++) {
  parseMsgpack();
}

// Timed run
const iterations = 1000;
const start = performance.now();
for (let i = 0; i < iterations; i++) {
  parseMsgpack();
}
const end = performance.now();
console.log(`RESULT_MS=${end - start}`);
