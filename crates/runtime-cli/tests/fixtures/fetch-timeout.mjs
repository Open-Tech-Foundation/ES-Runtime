// Bounding a request that the transport cannot bound for you. The client caps
// the connect phase, but a peer that accepts and then never answers is only
// ended by a caller-supplied deadline — the split this asserts.
import { serve } from "runtime:http";

const server = serve({ port: 0 }, (request) => {
  const { pathname } = new URL(request.url);
  // Never resolves: accepted, then silence.
  if (pathname === "/hang") return new Promise(() => {});
  return new Response("prompt");
});

const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

// A deadline ends a request the server would otherwise hold open forever.
const started = performance.now();
let name = "none";
try {
  await fetch(`${base}/hang`, { signal: AbortSignal.timeout(300) });
} catch (e) {
  name = e.name;
}
const elapsed = performance.now() - started;
console.log(`TIMEOUT name:${name} promptly:${elapsed < 5000}`);

// No deadline, no artificial cap: a normal request is unaffected.
console.log(`NORMAL body:${await (await fetch(base)).text()}`);

await server.stop();
console.log("TIMEOUT_OK");
