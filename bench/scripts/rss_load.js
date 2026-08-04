// Memory-under-load benchmark: build and hold a live working set while churning
// short-lived garbage around it, the shape a server has once it is actually
// serving — a session map or cache that stays reachable, with per-request
// objects allocated and dropped against it.
//
// The `rss` row on the site measures a near-empty process: the floor, which is
// the number a runtime looks best on and the one least like production. This row
// exists so peak resident set is also sampled while something is retained; see
// RSS_ROWS in run.sh. The elapsed time is reported too, since the allocation and
// collection work is itself a real cost, but the memory is the point.
(async () => {
  const LIVE = 200_000; // retained entries
  const CHURN = 20; // short-lived objects per retained one

  // Deliberately no in-process warmup: a warmup pass would either double the
  // retained set or leave a collected copy of it behind, and peak RSS cannot
  // tell the difference. The harness still discards a whole warmup *repetition*,
  // which is a separate process and does not perturb this one.
  const live = new Map();
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      // Retained: stays reachable for the whole run.
      live.set("session:" + i, {
        id: i,
        user: "user-" + (i % 5000),
        roles: ["reader", "writer"],
        seen: { at: i * 1000, ip: "10.0.0." + (i & 255) },
      });
      // Churned: allocated and dropped immediately, forcing collection work
      // against a heap that is mostly live.
      for (let j = 0; j < CHURN; j++) {
        const tmp = { k: i + j, tag: "req-" + j, buf: [j, j + 1, j + 2] };
        acc += tmp.buf.length;
      }
    }
    return acc;
  };

  const t0 = performance.now();
  const acc = run(LIVE);
  const t1 = performance.now();
  // Touch the retained set after timing so nothing above can be optimised away
  // and the working set is unambiguously still live at peak-RSS sampling time.
  if (live.size !== LIVE || acc === -1) console.log("unexpected", live.size, acc);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
