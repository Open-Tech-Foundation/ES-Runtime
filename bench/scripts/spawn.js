// Subprocess benchmark: spawn a trivial child, wait for it to exit, and collect
// its output — repeatedly. Process creation is not a shared Web API, so each
// runtime uses its own surface: `Deno.Command`, `Bun.spawn`, `node:child_process`,
// and esrun's `runtime:system` `Command`.
//
// `/bin/echo` rather than `/bin/true` because the output pipe is half the cost
// and the half that differs: fork/exec is the kernel's, but wiring up stdout,
// draining it, and handing it back as bytes is the runtime's. The payload is
// tiny so this stays a measure of the spawn path rather than of pipe throughput.
(async () => {
  const N = 200;
  const ARGS = ["benchmark-child-output"];
  const BIN = "/bin/echo";

  let spawnOnce;
  if (typeof Deno !== "undefined") {
    spawnOnce = async () => {
      const { stdout } = await new Deno.Command(BIN, { args: ARGS }).output();
      return stdout.length;
    };
  } else if (typeof Bun !== "undefined") {
    spawnOnce = async () => {
      const proc = Bun.spawn([BIN, ...ARGS], { stdout: "pipe" });
      const out = await new Response(proc.stdout).arrayBuffer();
      await proc.exited;
      return out.byteLength;
    };
  } else if (typeof process !== "undefined" && process.versions && process.versions.node) {
    // `spawn`, not `execFile`: LLRT ships the former and not the latter, and
    // reaching for the convenience wrapper would have recorded it as unable to
    // start a process at all — the same mistake the fs rows once made with
    // `unlink`. Both runtimes take this branch and both are measured.
    const { spawn } = await import("node:child_process");
    spawnOnce = () =>
      new Promise((resolve, reject) => {
        const child = spawn(BIN, ARGS, { stdio: ["ignore", "pipe", "ignore"] });
        let n = 0;
        child.stdout.on("data", (c) => {
          n += c.length;
        });
        child.on("error", reject);
        child.on("close", () => resolve(n));
      });
  } else {
    const { Command } = await import("runtime:system");
    spawnOnce = async () => {
      const { stdout } = await new Command(BIN, { args: ARGS }).output();
      return stdout.length;
    };
  }

  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc += await spawnOnce();
    return acc;
  };

  await run(Math.max(N / 10, 5)); // untimed warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
