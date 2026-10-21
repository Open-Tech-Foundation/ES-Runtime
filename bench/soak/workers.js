// Worker churn: start a worker, round-trip a message, terminate it, over and
// over, with several in flight at once. Each worker is an isolate and a thread,
// so a leak per worker shows up in memory and thread count within minutes.
const SCRIPT = new URL("./workers-echo.js", import.meta.url);
let done = 0;

async function loop() {
  for (;;) {
    const worker = new Worker(SCRIPT, { type: "module" });
    const reply = new Promise((resolve) => (worker.onmessage = (e) => resolve(e.data)));
    worker.postMessage(new Uint8Array(64 * 1024));
    if ((await reply) !== 64 * 1024) throw new Error("worker answered the wrong length");
    worker.terminate();
    done++;
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([loop(), loop(), loop(), loop()]);
