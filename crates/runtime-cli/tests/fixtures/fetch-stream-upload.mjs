// End-to-end streaming request body: a runtime:http echo server + a fetch with a
// lazily-produced ReadableStream body, over the real reqwest transport. Exercises
// chunked-transfer upload without buffering the whole payload.
import { serve } from "runtime:http";

const server = serve({ hostname: "127.0.0.1", port: 0 }, async (request) =>
  new Response(await request.text(), { status: 200 }),
);
const { port } = await server.addr;

const N = 500;
let i = 0;
const enc = new TextEncoder();
const body = new ReadableStream({
  pull(c) {
    if (i < N) c.enqueue(enc.encode(`chunk-${i++};`));
    else c.close();
  },
});

const res = await fetch(`http://127.0.0.1:${port}/`, { method: "POST", body });
const text = await res.text();

let expected = "";
for (let k = 0; k < N; k++) expected += `chunk-${k};`;

console.log(text === expected ? "UPLOAD_OK" : "UPLOAD_MISMATCH");
console.log(`status:${res.status}`);

await server.stop();
