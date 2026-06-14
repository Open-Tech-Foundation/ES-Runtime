// File write benchmark: create/truncate a 4 KB file in a loop. The filesystem
// is not a shared Web API, so each runtime uses its own surface.
(async () => {
  const N = 2000;
  const data = "x".repeat(4096);
  const tmp = "bench_fswrite.bin";

  let write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { unlink } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    cleanup = (p) => unlink(p).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    cleanup = (p) => fsp.unlink(p).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  const run = async (n) => {
    for (let i = 0; i < n; i++) await write(tmp, data);
  };
  await run(N / 10); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  await cleanup(tmp);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
