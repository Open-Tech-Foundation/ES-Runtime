// Same N as fsstat_small: `stat` reads metadata, so the file's size is the
// variable under test and the op count must not also change. At the N=20 the
// read/write `_large` workloads use (where 20 ops still move 40 MB) this
// measured 0.2-1.2 ms — timer resolution, published as a 6x ranking.
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
    const { stat: statP, unlink } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    stat = (p) => statP(p);
    cleanup = (p) => unlink(p).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    stat = (p) => fsp.stat(p);
    cleanup = (p) => fsp.unlink(p).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    stat = (p) => fs.stat(p);
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  await write(tmp, "x".repeat(2097152));
  const run = async (n) => {
    for (let i = 0; i < n; i++) await stat(tmp);
  };
  await run(N / 10);
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  await cleanup(tmp);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
