// End-to-end streaming response body: the server streams chunks produced over
// time (chunked transfer-encoding); the client must observe them incrementally
// — several reads, no Content-Length — rather than as one buffered payload.
import { serve } from "runtime:http";

const enc = new TextEncoder();
const N = 20;
const server = serve({ hostname: "127.0.0.1", port: 0 }, () => {
  let i = 0;
  const body = new ReadableStream({
    async pull(c) {
      if (i >= N) return c.close();
      // A real delay between chunks so arrival is necessarily incremental.
      await new Promise((r) => setTimeout(r, 5));
      c.enqueue(enc.encode(`tick-${i++};`));
    },
  });
  return new Response(body, { headers: { "x-mode": "stream" } });
});
const { port } = await server.addr;

const res = await fetch(`http://127.0.0.1:${port}/`);
const reader = res.body.getReader();
const dec = new TextDecoder();
let text = "";
let reads = 0;
for (;;) {
  const { value, done } = await reader.read();
  if (done) break;
  reads++;
  text += dec.decode(value, { stream: true });
}
text += dec.decode();

let expected = "";
for (let k = 0; k < N; k++) expected += `tick-${k};`;

console.log(text === expected ? "STREAM_OK" : `STREAM_MISMATCH:${text}`);
console.log(`reads>1:${reads > 1}`);
console.log(`content-length:${res.headers.get("content-length")}`);
console.log(`x-mode:${res.headers.get("x-mode")}`);

await server.stop();
