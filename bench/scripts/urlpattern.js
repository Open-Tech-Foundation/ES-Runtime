// URLPattern benchmark: parsing and matching complex routes.
(async () => {
  const N = 50_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const p = new URLPattern('/api/v1/users/:id/posts/:postId', 'https://api.example.com');
      if (p.test('https://api.example.com/api/v1/users/' + i + '/posts/' + (i + 1))) {
        acc++;
      }
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
