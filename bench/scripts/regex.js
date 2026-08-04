// Regex benchmark: the pattern work a server actually does on every request —
// matching a route, validating a field, and rewriting a string — over a fixed
// corpus. This is the engine's regex implementation (Irregexp for esrun, Node
// and Deno; JavaScriptCore's for Bun), which nothing else in the suite touches
// despite it running on essentially every inbound request in real code.
(async () => {
  const N = 200_000;

  const route = /^\/api\/v(\d+)\/users\/([a-f0-9-]{8})\/posts\/?$/;
  const email = /^[\w.+-]+@[\w-]+\.[\w.]{2,}$/;
  const collapse = /\s+/g;

  const paths = [
    "/api/v1/users/a1b2c3d4/posts",
    "/api/v2/users/deadbeef/posts/",
    "/api/v1/health",
    "/static/app.css",
  ];
  const emails = ["ops@example.com", "not-an-email", "a.b+c@sub.example.co.uk"];
  const messy = "  the   quick \t brown   fox  ";

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const m = route.exec(paths[i & 3]);
      if (m) acc += m[1].length + m[2].length;
      if (email.test(emails[i % 3])) acc++;
      acc += messy.replace(collapse, " ").length;
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
