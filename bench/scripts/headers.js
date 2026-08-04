// Header benchmark: build a request's worth of headers, read them back, append
// a couple, and iterate the lot — then do it again through a `Request`, which is
// the path an inbound request actually takes. Header handling happens on every
// single request a server answers, and the implementations differ a lot: a
// case-insensitive multi-map with ordering rules is more work than it looks.
(async () => {
  const N = 50_000;

  const base = [
    ["accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"],
    ["accept-encoding", "gzip, deflate, br"],
    ["accept-language", "en-US,en;q=0.9"],
    ["cache-control", "no-cache"],
    ["content-type", "application/json; charset=utf-8"],
    ["user-agent", "Mozilla/5.0 (X11; Linux x86_64) benchmark/1.0"],
    ["x-request-id", "0000-0000-0000-0000"],
  ];

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const h = new Headers(base);
      // Reads are case-insensitive lookups, not plain map gets.
      acc += h.get("Content-Type").length;
      acc += h.get("USER-AGENT").length;
      acc += h.has("x-request-id") ? 1 : 0;
      // Append, which must preserve order and combine duplicates.
      h.append("set-cookie", "a=1; Path=/");
      h.append("set-cookie", "b=2; Path=/");
      h.set("x-request-id", "req-" + i);
      // Iteration forces whatever lazy structure the runtime keeps.
      for (const [k, v] of h) acc += k.length + v.length;
      // The same headers arriving as a request.
      const req = new Request("http://127.0.0.1/api/v1/users/" + i, {
        method: "POST",
        headers: h,
      });
      acc += req.headers.get("accept-encoding").length;
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
