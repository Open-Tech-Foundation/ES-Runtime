// Encoding benchmark, large payloads: TextEncoder/TextDecoder round trips over
// a 64 KiB string rather than a 20-character one.
//
// The `encoding` row measures per-call cost — 100k round trips on a query
// string, where the payload is too small to matter and the number is dominated
// by the crossing itself. This measures the other half: transcoding throughput,
// which is what `fetch().text()` on a real response body or reading a file as
// text actually spends its time on. The two answer different questions and a
// runtime can be fast at one and slow at the other.
(async () => {
  const N = 1000;
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  // Mostly ASCII with a scattering of multi-byte characters, so neither the
  // pure-ASCII fast path nor the general decoder is the only thing measured.
  let text = "";
  while (text.length < 64 * 1024) {
    text += "the quick brown fox jumps over the lazy dog — å 0123456789 ";
  }
  text = text.slice(0, 64 * 1024);

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += dec.decode(enc.encode(text)).length;
    }
    return acc;
  };

  run(Math.max(N / 10, 5)); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
