// Stream a large CSV document through each runtime's streaming decoder in
// fixed-size byte chunks and count the rows. The other serialization rows bench
// `parse`, so this benches decode only — the encode side is the writer `build`
// already uses, not a second implementation.
//
// Each runtime uses the stream it actually ships: esrun's `CSV.DecoderStream`,
// Deno's `CsvParseStream` from `@std/csv`, and csv-parse's Node transform on
// Node and Bun, which ship no CSV reader at all.

// Generate a large CSV document (the same book catalogue as csv_large).
const rows = ["id,author,title,genre,price,publish_date,description"];
for (let i = 0; i < 50000; i++) {
  rows.push(
    `bk${i},"Gambardella, Matthew","XML Developer's Guide",Computer,44.95,2000-10-01,"An in-depth look at creating applications with XML."`,
  );
}
const csvText = rows.join("\n") + "\n";

// 64 KiB text chunks, so a field can span a boundary — the case the resumable
// scanner exists for. Text, not bytes: Deno's `CsvParseStream` and csv-parse
// both take string chunks, and esrun's decoder takes either.
const CHUNK = 65536;
const chunks = [];
for (let i = 0; i < csvText.length; i += CHUNK) {
  chunks.push(csvText.slice(i, i + CHUNK));
}

let mode = null;
let makeDecoder = null;
try {
  const mod = await import('runtime:serialization');
  makeDecoder = () => new mod.CSV.DecoderStream();
  mode = 'esrun';
} catch (e) {}
if (!mode && typeof Deno !== 'undefined') {
  const { CsvParseStream } = await import('jsr:@std/csv@1.0.6');
  makeDecoder = () => new CsvParseStream();
  mode = 'deno';
}
if (!mode) {
  const { parse } = await import('csv-parse');
  makeDecoder = () => parse();
  mode = 'node';
}

async function benchStream() {
  if (mode === 'node') {
    // csv-parse is a Node transform: write the chunks, count 'data' events.
    return new Promise((resolve, reject) => {
      const parser = makeDecoder();
      let count = 0;
      parser.on('data', () => count++);
      parser.on('end', () => resolve(count));
      parser.on('error', reject);
      for (const chunk of chunks) parser.write(chunk);
      parser.end();
    });
  }
  // esrun's CSV.DecoderStream and Deno's CsvParseStream are Web TransformStreams:
  // write text chunks in, read decoded rows out.
  const decoder = makeDecoder();
  const writer = decoder.writable.getWriter();
  const reader = decoder.readable.getReader();
  const writing = (async () => {
    for (const chunk of chunks) await writer.write(chunk);
    await writer.close();
  })();
  let count = 0;
  while (true) {
    const { done } = await reader.read();
    if (done) break;
    count++;
  }
  await writing;
  return count;
}

async function run() {
  for (let i = 0; i < 2; i++) {
    await benchStream();
  }
  const start = performance.now();
  for (let i = 0; i < 5; i++) {
    await benchStream();
  }
  const end = performance.now();
  console.log(`RESULT_MS=${end - start}`);
}

await run();
