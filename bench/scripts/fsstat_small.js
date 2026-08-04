(async () => {
  const N = 5000;
  const tmp = "bench_fsstat.bin";

  let stat, write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    stat = (p) => Deno.stat(p);
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { stat: statP, rm } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    stat = (p) => statP(p);
    cleanup = (p) => rm(p, { force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    stat = (p) => fsp.stat(p);
    cleanup = (p) => fsp.rm(p, { force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    stat = (p) => fs.stat(p);
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  await write(tmp, "x".repeat(4096));
  const run = async (n) => {
    for (let i = 0; i < n; i++) await stat(tmp);
  };
  await run(N / 10);
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  // Reported before cleanup: a teardown failure must not discard a measurement
  // that already succeeded — a missing result reaches the site as "unsupported".
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
  await cleanup(tmp);
})();
