// Compression benchmark: CompressionStream -> DecompressionStream pipe
// moving 1 KiB chunks. Exercises the Compression Streams API (gzip format).
(async () => {
  const CHUNKS = 20_000;
  const chunk = new Uint8Array(1024).fill(120); // 1 KiB of repeating 'x'
  
  const run = async (n, format) => {
    let total = 0;
    let i = 0;
    
    const source = new ReadableStream({
      pull(controller) {
        if (i++ < n) {
          controller.enqueue(chunk);
        } else {
          controller.close();
        }
      },
    });
    
    const compress = new CompressionStream(format);
    const decompress = new DecompressionStream(format);
    
    const sink = new WritableStream({
      write(c) {
        total += c.length;
      },
    });
    
    await source
      .pipeThrough(compress)
      .pipeThrough(decompress)
      .pipeTo(sink);
      
    return total;
  };

  // Warmup
  await run(100, "gzip");
  
  const t0 = performance.now();
  const total = await run(CHUNKS, "gzip");
  const t1 = performance.now();
  
  if (total === -1) console.log(total); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
