// Fetch benchmark: sequential GETs against a local HTTP server (started by
// run.sh on a fixed port; the workload is skipped if it isn't running). The
// only workload that exercises the network provider seam end-to-end: request
// marshaling, the transport, response streaming, and body reads.
(async () => {
  const URL_ = "http://127.0.0.1:18923/";
  const N = 300;
  const run = async (n) => {
    let total = 0;
    for (let i = 0; i < n; i++) {
      const res = await fetch(URL_);
      total += (await res.text()).length;
    }
    return total;
  };
  await run(N / 10); // warmup (also primes connection pools)
  const t0 = performance.now();
  const total = await run(N);
  const t1 = performance.now();
  if (total === -1) console.log(total); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
