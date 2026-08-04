// File append benchmark: append 256 KB to a growing file in a loop. The
// filesystem is not a shared Web API, so each runtime uses its own surface.
//
// Sizing this row is a squeeze between two failure modes. It was 2 MB x 20,
// growing the file to 42 MB per launch and writing ~1 GB across a full row: past
// the kernel's dirty-page threshold, whoever was running when writeback landed
// wore the stall, and the published minimum became a lottery — bun's floor once
// sat 668% below the next-lowest sample it produced. Shrink too far and the
// opposite happens: at 128 KB the fastest runtimes finish in under 3 ms and the
// row measures the clock. 256 KB x 60 holds the file to 15 MB, which stays in
// page cache, and keeps every runtime at or above the measurement floor with
// each one's minimum corroborated to within 6%.
//
// Note this measures the append path into page cache — nothing here fsyncs, so
// it is not a durability measurement.
(async () => {
  const N = 60;
  const data = "x".repeat(262144);
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
  await run(1); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  // Reported before cleanup: a teardown failure must not discard a measurement
  // that already succeeded — a missing result reaches the site as "unsupported".
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
  await cleanup(tmp);
})();
