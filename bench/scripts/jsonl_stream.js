let isEsrun = false;
let JSONLDecoderStream, JSONLEncoderStream;
try {
  const parsers = await import("runtime:parsers");
  JSONLDecoderStream = parsers.JSONL.DecoderStream;
  JSONLEncoderStream = parsers.JSONL.EncoderStream;
  isEsrun = true;
} catch (e) {
}

// Generate data
const records = [];
for (let i = 0; i < 50000; i++) {
  records.push({ id: i, name: "TestUser" + i, active: true, createdAt: "2026-06-20T10:00:00Z" });
}

let ndjson;
if (!isEsrun) {
  ndjson = (await import("ndjson")).default || await import("ndjson");
}

async function benchEsrun() {
  const encoder = new JSONLEncoderStream();
  const decoder = new JSONLDecoderStream();
  
  // Pipeline: encoder -> decoder
  encoder.readable.pipeTo(decoder.writable);
  
  const writer = encoder.writable.getWriter();
  const reader = decoder.readable.getReader();
  
  const writePromise = (async () => {
    for (let i = 0; i < records.length; i++) {
      await writer.write(records[i]);
    }
    await writer.close();
  })();
  
  const readPromise = (async () => {
    let count = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      count++;
    }
  })();
  
  await Promise.all([writePromise, readPromise]);
}

async function benchNode() {
  const serialize = ndjson.stringify();
  const parse = ndjson.parse();
  
  serialize.pipe(parse);
  
  const writePromise = new Promise((resolve) => {
    for (let i = 0; i < records.length; i++) {
      serialize.write(records[i]);
    }
    serialize.end();
    resolve();
  });
  
  const readPromise = new Promise((resolve, reject) => {
    let count = 0;
    parse.on('data', (obj) => {
      count++;
    });
    parse.on('end', () => {
      resolve();
    });
    parse.on('error', reject);
  });
  
  await Promise.all([writePromise, readPromise]);
}

async function run() {
  // Warmup
  for (let i = 0; i < 2; i++) {
    if (isEsrun) await benchEsrun();
    else await benchNode();
  }
  
  const start = performance.now();
  for (let i = 0; i < 5; i++) {
    if (isEsrun) await benchEsrun();
    else await benchNode();
  }
  const end = performance.now();
  console.log(`RESULT_MS=${end - start}`);
}

await run();
