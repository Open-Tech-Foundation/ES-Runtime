// Binary-buffer benchmark: TypedArray and DataView work over a 64 KiB block —
// copying regions, reading and writing big-endian fields, and taking subarray
// views. This is the layer every binary protocol sits on (framing a WebSocket
// message, decoding a database wire format), and it is measured only indirectly
// elsewhere, underneath a parser.
(async () => {
  const SIZE = 65536;
  const src = new Uint8Array(SIZE);
  for (let i = 0; i < SIZE; i++) src[i] = i & 0xff;
  const dst = new Uint8Array(SIZE);
  const view = new DataView(src.buffer);

  // 100k, not 20k: at 20k every runtime but LLRT finished in under 5 ms, which
  // is the harness's measurement floor — close enough to timer resolution that
  // the ranking was partly noise. The work is unchanged in kind, only in amount.
  const N = 100_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      // Bulk copy, the memcpy path.
      dst.set(src.subarray(0, 4096), (i & 7) * 4096);
      // Field reads/writes, the framing path.
      const off = (i & 255) * 4;
      view.setUint32(off, i, false);
      acc += view.getUint32(off, false) & 0xff;
      acc += view.getUint16(off, true);
      // A view, not a copy — cheap if the runtime does it right.
      acc += src.subarray(off, off + 16).length;
    }
    return acc;
  };

  run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
