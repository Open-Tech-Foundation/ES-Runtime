// File append benchmark: append 4 KB to a growing file in a loop. The
// filesystem is not a shared Web API, so each runtime uses its own surface.
(async () => {
  const N = 2000;
  const data = "x".repeat(4096);
  const tmp = "bench_fsappend.bin";

  let append, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    append = (p, d) => Deno.writeFile(p, enc.encode(d), { append: true });
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { appendFile, rm } = await import("node:fs/promises");
    append = (p, d) => appendFile(p, d);
    cleanup = (p) => rm(p, { force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    append = (p, d) => fsp.appendFile(p, d);
    cleanup = (p) => fsp.rm(p, { force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    append = (p, d) => fs.write(p, d, { append: true });
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  await cleanup(tmp); // start fresh
  const run = async (n) => {
    for (let i = 0; i < n; i++) await append(tmp, data);
  };
  await run(N / 10); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  // Reported before cleanup: a teardown failure must not discard a measurement
  // that already succeeded — a missing result reaches the site as "unsupported".
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
  await cleanup(tmp);
})();
