(async () => {
  const N = 5000;
  const tmp = "bench_fsexists.bin";

  let exists, write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    exists = (p) => Deno.stat(p).then(()=>true).catch(()=>false);
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { stat, rm } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    exists = (p) => stat(p).then(()=>true).catch(()=>false);
    cleanup = (p) => rm(p, { force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    exists = (p) => fsp.stat(p).then(()=>true).catch(()=>false);
    cleanup = (p) => fsp.rm(p, { force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    exists = (p) => fs.exists(p);
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  await write(tmp, "x".repeat(4096));
  const run = async (n) => {
    for (let i = 0; i < n; i++) await exists(tmp);
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
