// Isolate churn: create, call into, message and terminate Isolates, several at
// once, with some ended by their CPU or memory ceiling. Isolates share a pool
// of threads, so a leak per isolate shows in memory and the pool's threads
// should stay flat however many come and go.
import { Isolate } from "runtime:workers";

const SOURCE = `
  import { host } from "runtime:workers";
  export async function add(a, b) { return (await host.twice(a)) + b; }
  export function spin() { for (;;) {} }
  export function hoard() { const keep = []; for (;;) keep.push(new Array(1e5).fill(1)); }
  globalThis.onmessage = (e) => globalThis.postMessage(e.data.length);
`;
const host = { twice: (n) => n * 2 };
let done = 0;

async function calls() {
  for (let i = 0; ; i++) {
    const iso = new Isolate({ source: SOURCE }, { host, memory: 16, cpuTime: 50 });
    const { add } = await iso.exports;
    if ((await add(i, 1)) !== i * 2 + 1) throw new Error("isolate answered wrong");
    const reply = new Promise((resolve) => iso.addEventListener("message", (e) => resolve(e.data), { once: true }));
    iso.postMessage("x".repeat(4096));
    if ((await reply) !== 4096) throw new Error("isolate echoed the wrong length");
    iso.terminate();
    done++;
  }
}

async function limits() {
  for (let i = 0; ; i++) {
    const iso = new Isolate({ source: SOURCE }, { host, memory: 16, cpuTime: 50 });
    const exports = await iso.exports;
    const expected = i % 2 === 0 ? "ERR_ISOLATE_CPU_LIMIT" : "ERR_ISOLATE_MEMORY_LIMIT";
    // The limit's own error is also reported to the isolate's error handler.
    iso.onerror = (e) => e.preventDefault();
    try {
      await (i % 2 === 0 ? exports.spin() : exports.hoard());
      throw new Error(`${expected} did not happen`);
    } catch (e) {
      if (e.code !== expected) throw e;
    }
    done++;
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([calls(), calls(), calls(), limits()]);
