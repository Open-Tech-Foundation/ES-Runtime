// File I/O benchmark: write a small file and read it back in a tight loop —
// measures whole-file write+read throughput. Unlike the other workloads, the
// filesystem is not a shared Web API, so each runtime uses its own surface
// (esrun: runtime:fs; Node: node:fs/promises; Bun: Bun.file/write; Deno: Deno).
(async () => {
  const N = 2000;
  const data = "x".repeat(4096); // 4 KB payload
  const tmp = "bench_fs_tmp.bin";

  let write, read, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    read = (p) => Deno.readFile(p);
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { unlink } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    read = (p) => Bun.file(p).arrayBuffer();
    cleanup = (p) => unlink(p).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    read = (p) => fsp.readFile(p);
    cleanup = (p) => fsp.unlink(p).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    write = (p, d) => fs.write(p, d);
    read = (p) => fs.file(p).arrayBuffer();
    cleanup = (p) => fs.remove(p).catch(() => {});
  }

  const cycle = async (n) => {
    for (let i = 0; i < n; i++) {
      await write(tmp, data);
      await read(tmp);
    }
  };

  await cycle(N / 10); // untimed JIT/cache warmup
  const t0 = performance.now();
  await cycle(N);
  const t1 = performance.now();
  await cleanup(tmp);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
