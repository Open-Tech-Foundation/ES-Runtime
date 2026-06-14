// Glob benchmark: scan a small generated tree for `**/*.txt`, repeated. Glob is
// runtime-specific; Deno has no built-in runtime glob, so it is skipped (n/a).
(async () => {
  if (typeof Deno !== "undefined") return; // no built-in runtime glob

  const DIRS = 10;
  const FILES = 10;
  const SCANS = 200;
  const root = "bench_glob_tree";

  let mkdir, write, scan, cleanup;
  if (typeof Bun !== "undefined") {
    const { mkdir: nmkdir, rm } = await import("node:fs/promises");
    mkdir = (p) => nmkdir(p, { recursive: true });
    write = (p, d) => Bun.write(p, d);
    scan = async (pat, cwd) => {
      let c = 0;
      for await (const _ of new Bun.Glob(pat).scan({ cwd })) c++;
      return c;
    };
    cleanup = (p) => rm(p, { recursive: true, force: true });
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    const fsp = await import("node:fs/promises");
    if (typeof fsp.glob !== "function") return; // older Node: no fs.glob
    mkdir = (p) => fsp.mkdir(p, { recursive: true });
    write = (p, d) => fsp.writeFile(p, d);
    scan = async (pat, cwd) => {
      let c = 0;
      for await (const _ of fsp.glob(pat, { cwd })) c++;
      return c;
    };
    cleanup = (p) => fsp.rm(p, { recursive: true, force: true });
  } else {
    const fs = await import("runtime:fs");
    mkdir = (p) => fs.mkdir(p, { recursive: true });
    write = (p, d) => fs.write(p, d);
    scan = async (pat, cwd) => {
      let c = 0;
      for await (const _ of new fs.Glob(pat).scan({ cwd })) c++;
      return c;
    };
    cleanup = (p) => fs.remove(p, { recursive: true }).catch(() => {});
  }

  await cleanup(root);
  for (let d = 0; d < DIRS; d++) {
    await mkdir(root + "/d" + d);
    for (let f = 0; f < FILES; f++) await write(root + "/d" + d + "/f" + f + ".txt", "x");
  }

  await scan("**/*.txt", root); // untimed warmup
  const t0 = performance.now();
  let total = 0;
  for (let i = 0; i < SCANS; i++) total += await scan("**/*.txt", root);
  const t1 = performance.now();
  await cleanup(root);
  if (total < 0) console.log(total); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
