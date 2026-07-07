// Proxy/echo shape: the request body stream is passed straight through as the
// response body — `new Response(request.body)` — so one request concurrently
// pulls inbound chunks and pushes them back out, with nothing buffered on
// either side.
import { serve } from "runtime:http";

const server = serve({ hostname: "127.0.0.1", port: 0 }, (request) => new Response(request.body));
const { port } = await server.addr;

const N = 100;
let i = 0;
const enc = new TextEncoder();
const body = new ReadableStream({
  pull(c) {
    if (i < N) c.enqueue(enc.encode(`echo-${i++};`));
    else c.close();
  },
});

const res = await fetch(`http://127.0.0.1:${port}/`, { method: "POST", body });
const text = await res.text();

let expected = "";
for (let k = 0; k < N; k++) expected += `echo-${k};`;

console.log(text === expected ? "ECHO_OK" : "ECHO_MISMATCH");
console.log(`status:${res.status}`);

await server.stop();
