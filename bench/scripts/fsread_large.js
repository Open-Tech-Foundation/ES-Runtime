// File read benchmark: read a 2 MB file in a loop (written once, untimed). The
// filesystem is not a shared Web API, so each runtime uses its own surface.
// N is 60 rather than 20 because at 20 the fastest runtime (LLRT) finished in
// 4.8 ms — under the harness's measurement floor, where the ranking is partly
// timer resolution. Reads are page-cache served and allocate no lasting bytes,
// so tripling the count costs a few milliseconds and none of the writeback
// pressure that constrains the append rows.
(async () => {
  const N = 60;
  const data = "x".repeat(2097152);
  const tmp = "bench_fsread.bin";

  let write, read, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    read = (p) => Deno.readFile(p);
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { rm } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    read = (p) => Bun.file(p).arrayBuffer();
    cleanup = (p) => rm(p, { force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    read = (p) => fsp.readFile(p);
    cleanup = (p) => fsp.rm(p, { force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    read = (p) => fs.file(p).arrayBuffer();
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  await write(tmp, data); // setup, untimed
  const run = async (n) => {
    for (let i = 0; i < n; i++) await read(tmp);
  };
  await run(1); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  // Reported before cleanup: a teardown failure must not discard a measurement
  // that already succeeded — a missing result reaches the site as "unsupported".
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
  await cleanup(tmp);
})();
