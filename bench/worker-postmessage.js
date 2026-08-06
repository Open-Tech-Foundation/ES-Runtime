// Worker `postMessage` round-trip cost, by payload size.
//
// Run:  esrun bench/worker-postmessage.js
//
// Why this exists: the message path copies the serialized payload once on the
// JS→Rust op crossing (`Value::Bytes`, ARCHITECTURE §9 / the D3a Phase 8
// deferral). Removing that copy means giving the whole provider seam a byte
// container that can own a V8 backing store, which is a workspace-wide change —
// so it wants a number attached rather than an intuition.
//
// Read it as: per-message cost at 1 KiB is fixed overhead (two op crossings, a
// promise, a tick, a thread wake-up), where a memcpy of 1 KiB is ~0.1 µs. The
// copy only becomes visible in the megabyte rows.
const worker = new URL("./worker-postmessage-echo.js", import.meta.url);

const SIZES = [1024, 64 * 1024, 1024 * 1024, 8 * 1024 * 1024];
const ROUNDS = 200;

const w = new Worker(worker);
let index = 0;

function measure() {
  if (index >= SIZES.length) {
    w.terminate();
    return;
  }
  const size = SIZES[index];
  const payload = new Uint8Array(size);
  let seen = 0;
  const started = performance.now();

  w.onmessage = () => {
    if (++seen < ROUNDS) {
      w.postMessage(payload);
      return;
    }
    const elapsed = performance.now() - started;
    const mib = (size * ROUNDS) / (1024 * 1024);
    console.log(
      `${String(size / 1024).padStart(5)} KiB x${ROUNDS}: ` +
        `${(elapsed / ROUNDS).toFixed(3)} ms/msg, ` +
        `${(mib / (elapsed / 1000)).toFixed(0)} MiB/s`,
    );
    index++;
    measure();
  };
  w.postMessage(payload);
}

measure();
