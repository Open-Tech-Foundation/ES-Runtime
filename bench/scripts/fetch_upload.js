// fetch streamed-upload benchmark: sequential POSTs that stream a ReadableStream
// request body to the local server (started by run.sh on a fixed port; the
// workload is skipped if it isn't running). Exercises the request-body streaming
// path end to end — building the body stream, the per-chunk host channel with
// backpressure, and chunked transfer-encoding — as opposed to the buffered-body
// `fetch` workload. `duplex: "half"` is required by some runtimes (Node/undici)
// to send a stream body and ignored by the rest.
(async () => {
  const URL_ = "http://127.0.0.1:18923/upload";
  const N = 200;
  const CHUNKS = 8;
  const chunk = new Uint8Array(1024).fill(120); // 1 KB of 'x'
  const makeBody = () => {
    let i = 0;
    return new ReadableStream({
      pull(c) {
        if (i++ < CHUNKS) c.enqueue(chunk);
        else c.close();
      },
    });
  };
  const EXPECT = CHUNKS * chunk.length; // bytes the server must receive per POST
  const run = async (n) => {
    let total = 0;
    for (let i = 0; i < n; i++) {
      const res = await fetch(URL_, {
        method: "POST",
        body: makeBody(),
        duplex: "half",
      });
      // The server echoes the number of body bytes it received. Verify the body
      // truly streamed — a runtime that drops/coerces the stream body instead of
      // uploading it must NOT post a (misleadingly fast) time. Throwing here means
      // no RESULT_MS is printed, so the harness records this runtime as n/a.
      const got = Number(await res.text());
      if (got !== EXPECT) {
        throw new Error(`upload not streamed: server got ${got}, expected ${EXPECT}`);
      }
      total += got;
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
