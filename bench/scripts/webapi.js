// Web-API benchmark: URL parsing + TextEncoder/TextDecoder in a loop. For esrun
// each URL parse crosses the JS↔Rust op boundary (the `url` crate); Node/Deno/
// Bun parse natively. Measures that surface plus UTF-8 transcoding.
(async () => {
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const N = 100_000;
  const t0 = performance.now();
  let n = 0;
  for (let i = 0; i < N; i++) {
    const u = new URL("https://example.com/a/b?q=" + i + "&lang=en#frag");
    const bytes = enc.encode(u.pathname + u.search);
    n += dec.decode(bytes).length;
  }
  const t1 = performance.now();
  if (n === -1) console.log(n);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
