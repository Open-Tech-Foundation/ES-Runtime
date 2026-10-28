// BSON.decode's cost and what it leaves behind: the time to decode 20,000
// documents, to decode and then use them as a server does, and the heap one
// decode holds. Decoded objects are built so V8 keeps them in fast mode
// (DECISIONS D187, update 2026-10-28); this is the script that measures it.
//
//   target/release/esrun bench/bson/decode.mjs
//
// Each row is the minimum of RUNS timed runs after a warmup.
import { BSON } from "runtime:serialization";
import { memoryUsage } from "runtime:process";

const RUNS = 10;
const COUNT = 20000;

// The shape of a typical collection: an ObjectId and a handful of fields.
const documents = BSON.encode({
  docs: Array.from({ length: COUNT }, (_, i) => ({
    _id: new BSON.ObjectId(),
    id: i,
    name: `user ${i}`,
    email: `u${i}@example.com`,
    score: i * 1.5,
    active: i % 2 === 0,
    tags: ["a", "b"],
  })),
});
// The same without class values: the case that pays most for building each
// object a property at a time.
const plain = BSON.encode({
  docs: Array.from({ length: COUNT }, (_, i) => ({ id: i, name: `user ${i}`, score: i * 1.5, active: i % 2 === 0, tags: ["a", "b"] })),
});

const rows = {
  "decode": () => BSON.decode(documents),
  "decode + JSON.stringify": () => JSON.stringify(BSON.decode(documents)),
  "decode + map to API shape": () =>
    BSON.decode(documents).docs.map((d) => ({ id: d._id.toHexString(), name: d.name, score: d.score })),
  "decode, plain documents": () => BSON.decode(plain),
};

for (const [label, run] of Object.entries(rows)) {
  run();
  let best = Infinity;
  for (let i = 0; i < RUNS; i++) {
    const start = performance.now();
    run();
    best = Math.min(best, performance.now() - start);
  }
  console.log(`${label.padEnd(28)} ${best.toFixed(1)} ms`);
}

const decoded = BSON.decode(documents).docs;
const read = () => {
  let sum = 0;
  for (const d of decoded) sum += d.score + d.id + d.tags.length;
  return sum;
};
read();
let best = Infinity;
for (let i = 0; i < RUNS; i++) {
  const start = performance.now();
  for (let k = 0; k < 10; k++) read();
  best = Math.min(best, performance.now() - start);
}
console.log(`${"read 3 fields x 10 passes".padEnd(28)} ${best.toFixed(2)} ms`);

const before = memoryUsage().heapUsed;
const held = BSON.decode(documents);
console.log(`${"heap held by one decode".padEnd(28)} ${((memoryUsage().heapUsed - before) / 1e6).toFixed(1)} MB (${held.docs.length} documents)`);
