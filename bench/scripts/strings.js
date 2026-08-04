// String benchmark: building and slicing strings the way request handling does —
// template interpolation, concatenation into a buffer, splitting a header, case
// folding, and substring search. Plausibly the most-executed shape of code in
// any web server, and measured nowhere else here: `json` exercises the
// serializer, `encoding` the UTF-8 boundary, neither the string internals
// (ropes, slices, interning) these depend on.
(async () => {
  const N = 100_000;

  const header = "text/html; charset=utf-8; boundary=--abc123";
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      // Template interpolation into a log-shaped line.
      const line = `GET /users/${i} 200 ${i * 3}ms`;
      // Concatenation, the rope-building path.
      let buf = "";
      for (let j = 0; j < 8; j++) buf += line;
      // Split + trim, the header-parsing path.
      const parts = header.split(";");
      acc += parts.length + parts[0].trim().length;
      // Search and slice.
      const at = buf.indexOf("200");
      acc += at >= 0 ? buf.slice(at, at + 3).length : 0;
      acc += line.toUpperCase().length;
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
