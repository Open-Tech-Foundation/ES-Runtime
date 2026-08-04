// Existence-check benchmark across many distinct paths: probe 1000 different
// files in a directory, repeatedly. The filesystem is not a shared Web API, so
// each runtime uses its own surface.
//
// This replaces `fsexists_large`, which differed from `fsexists_small` only by
// writing a 2 MB file instead of a 4 KB one before probing it the same number of
// times. An existence check reads metadata and moves no bytes, so file size is
// not a variable it has, and the two rows duly agreed to within 0.2% (node 70.6
// vs 70.5, bun 51.2 vs 51.0, deno 90.6 vs 90.6) — two charted rows carrying one
// row of information. Path *count* is a real second dimension: it walks a
// directory's worth of dentries instead of hitting one cached entry over and
// over, which is what a static-file server or a module resolver actually does.
//
// The check is deliberately NOT `stat().then(true).catch(false)` on every
// runtime. That is what this row used to do on Node, Bun and Deno, which made
// it a near-duplicate of the fsstat rows — the same syscall plus a promise.
// Node and LLRT use `access()` (faccessat answers the question without filling
// in a stat buffer), Bun its native `Bun.file().exists()`, esrun `runtime:fs`
// `exists()`. Deno ships no existence primitive, so stat there is the idiomatic
// answer rather than a shortcut, and that one cell still measures a stat.
(async () => {
  const FILES = 1000;
  const ROUNDS = 20;
  const dir = "bench_fsexists_many";

  let exists, mkdir, write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    mkdir = (p) => Deno.mkdir(p, { recursive: true });
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    // Deno ships no existence primitive; stat is the idiomatic check.
    exists = (p) => Deno.stat(p).then(() => true).catch(() => false);
    cleanup = (p) => Deno.remove(p, { recursive: true }).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { mkdir: nmkdir, rm } = await import("node:fs/promises");
    mkdir = (p) => nmkdir(p, { recursive: true });
    write = (p, d) => Bun.write(p, d);
    exists = (p) => Bun.file(p).exists();
    cleanup = (p) => rm(p, { recursive: true, force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    mkdir = (p) => fsp.mkdir(p, { recursive: true });
    write = (p, d) => fsp.writeFile(p, d);
    exists = (p) => fsp.access(p).then(() => true).catch(() => false);
    cleanup = (p) => fsp.rm(p, { recursive: true, force: true }).catch(() => {});
  } else {
    const fs = await import("runtime:fs");
    mkdir = (p) => fs.mkdir(p, { recursive: true });
    write = (p, d) => fs.write(p, d);
    exists = (p) => fs.exists(p);
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
      for (let i = 0; i < FILES; i++) await exists(paths[i]);
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
