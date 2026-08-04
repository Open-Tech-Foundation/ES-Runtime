// Metadata benchmark across many distinct paths: stat 1000 different files in a
// directory, repeatedly. The filesystem is not a shared Web API, so each runtime
// uses its own surface.
//
// This replaces `fsstat_large`, which differed from `fsstat_small` only by
// writing a 2 MB file instead of a 4 KB one before stat'ing it the same number
// of times. `stat` reads an inode and moves no bytes, so file size is not a
// variable it has, and the two rows duly agreed to within 4% (node 68.7 vs 71.6,
// bun 49.8 vs 50.7, deno 92.4 vs 94.7) — two charted rows carrying one row of
// information. Path *count* is a real second dimension: it walks a directory's
// worth of dentries instead of hitting one cached entry over and over, which is
// what a static-file server or a module resolver actually does.
(async () => {
  const FILES = 1000;
  const ROUNDS = 20;
  const dir = "bench_fsstat_many";

  let stat, mkdir, write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    mkdir = (p) => Deno.mkdir(p, { recursive: true });
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    stat = (p) => Deno.stat(p);
    cleanup = (p) => Deno.remove(p, { recursive: true }).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { stat: statP, mkdir: nmkdir, rm } = await import("node:fs/promises");
    mkdir = (p) => nmkdir(p, { recursive: true });
    write = (p, d) => Bun.write(p, d);
    stat = (p) => statP(p);
    cleanup = (p) => rm(p, { recursive: true, force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    mkdir = (p) => fsp.mkdir(p, { recursive: true });
    write = (p, d) => fsp.writeFile(p, d);
    stat = (p) => fsp.stat(p);
    cleanup = (p) => fsp.rm(p, { recursive: true, force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    mkdir = (p) => fs.mkdir(p, { recursive: true });
    write = (p, d) => fs.write(p, d);
    stat = (p) => fs.stat(p);
    cleanup = (p) => fs.remove(p, { recursive: true }).catch(() => {});
  }

  await cleanup(dir);
  await mkdir(dir);
  const paths = [];
  for (let i = 0; i < FILES; i++) {
    const p = `${dir}/f${i}.bin`;
    paths.push(p);
    await write(p, "x");
  }

  const run = async (rounds) => {
    for (let r = 0; r < rounds; r++) {
      for (let i = 0; i < FILES; i++) await stat(paths[i]);
    }
  };
  await run(Math.max(ROUNDS / 10, 1)); // untimed warmup
  const t0 = performance.now();
  await run(ROUNDS);
  const t1 = performance.now();
  // Reported before cleanup: a teardown failure must not discard a measurement
  // that already succeeded — a missing result reaches the site as "unsupported".
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
  await cleanup(dir);
})();
