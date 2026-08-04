// Multipart benchmark: encode a `FormData` into a request body and parse it
// back out, which is what a file upload or an HTML form POST costs a server.
// The round trip is deliberate — `Request.formData()` has to find the boundary,
// split the parts, and decode each one's headers, and that parse is the half
// that runs on untrusted input in production.
(async () => {
  const N = 2_000;
  const fileBody = "x".repeat(4096);

  const build = () => {
    const fd = new FormData();
    fd.append("title", "benchmark upload");
    fd.append("description", "a field long enough to not be a single chunk ".repeat(3));
    fd.append("tags", "alpha,beta,gamma");
    fd.append("file", new Blob([fileBody], { type: "text/plain" }), "payload.txt");
    return fd;
  };

  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const req = new Request("http://127.0.0.1/upload", {
        method: "POST",
        body: build(),
      });
      const parsed = await req.formData();
      acc += parsed.get("title").length;
      const f = parsed.get("file");
      acc += typeof f === "string" ? f.length : f.size;
    }
    return acc;
  };

  await run(N / 10); // untimed warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
