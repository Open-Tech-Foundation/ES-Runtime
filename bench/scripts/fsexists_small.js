// Existence-check benchmark: does this path exist, asked 5 000 times of one
// path via each runtime's idiomatic API. The filesystem is not a shared Web
// API, so each runtime uses its own surface.
//
// The check is deliberately NOT `stat().then(true).catch(false)` on every
// runtime. That is what this row used to do on Node, Bun and Deno, which made
// it a near-duplicate of the fsstat rows — the same syscall plus a promise.
// Node and LLRT use `access()` (faccessat answers the question without filling
// in a stat buffer), Bun its native `Bun.file().exists()`, esrun `runtime:fs`
// `exists()`. Deno ships no existence primitive, so stat there is the idiomatic
// answer rather than a shortcut, and that one cell still measures a stat.
(async () => {
  const N = 5000;
  const tmp = "bench_fsexists.bin";

  let exists, write, cleanup;
  if (typeof Deno !== "undefined") {
    const enc = new TextEncoder();
    write = (p, d) => Deno.writeFile(p, enc.encode(d));
    // Deno ships no existence primitive; stat is the idiomatic check.
    exists = (p) => Deno.stat(p).then(() => true).catch(() => false);
    cleanup = (p) => Deno.remove(p).catch(() => {});
  } else if (typeof Bun !== "undefined") {
    const { rm } = await import("node:fs/promises");
    write = (p, d) => Bun.write(p, d);
    exists = (p) => Bun.file(p).exists();
    cleanup = (p) => rm(p, { force: true }).catch(() => {});
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    write = (p, d) => fsp.writeFile(p, d);
    exists = (p) => fsp.access(p).then(() => true).catch(() => false);
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
