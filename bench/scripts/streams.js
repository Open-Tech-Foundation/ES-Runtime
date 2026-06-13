// Streams benchmark: ReadableStream → TransformStream → WritableStream pipe
// moving 1 KiB chunks. Exercises the whole streams machinery (pull scheduling,
// queuing strategies, pipe plumbing) — for esrun that's the pure-JS prelude.
(async () => {
  const CHUNKS = 5_000;
  const chunk = new Uint8Array(1024).fill(120);
  const run = async (n) => {
    let total = 0;
    let i = 0;
    const source = new ReadableStream({
      pull(controller) {
        if (i++ < n) controller.enqueue(chunk);
        else controller.close();
      },
    });
    const transform = new TransformStream({
      transform(c, controller) {
        controller.enqueue(c);
      },
    });
    const sink = new WritableStream({
      write(c) {
        total += c.length;
      },
    });
    await source.pipeThrough(transform).pipeTo(sink);
    return total;
  };
  await run(CHUNKS / 10); // untimed warmup
  const t0 = performance.now();
  const total = await run(CHUNKS);
  const t1 = performance.now();
  if (total === -1) console.log(total); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
