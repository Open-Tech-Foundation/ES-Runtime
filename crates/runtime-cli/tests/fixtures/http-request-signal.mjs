// The handler's `request.signal` over a real connection: it aborts when the
// client hangs up mid-request, and stays unaborted when the request completes
// normally. Only the driven CLI can show this — it needs a real socket to close.
import { serve } from "runtime:http";

const seen = { abortedReason: null, completedSignal: null };

const server = serve({ port: 0 }, async (request) => {
  const { pathname } = new URL(request.url);

  if (pathname === "/slow") {
    // Wait for either the client to go away or a generous deadline. A handler
    // that could not tell the difference would always sit out the full delay.
    const signal = request.signal;
    await new Promise((resolve) => {
      const timer = setTimeout(resolve, 5000);
      signal.addEventListener(
        "abort",
        () => {
          seen.abortedReason = signal.reason?.name ?? "unknown";
          clearTimeout(timer);
          resolve();
        },
        { once: true },
      );
    });
    return new Response("slow done");
  }

  if (pathname === "/quick") {
    seen.completedSignal = request.signal;
    return new Response("quick done");
  }

  // Never touches request.signal: the lazy path, which must start no watch.
  return new Response("plain done");
});

const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

// A handler that ignores the signal is unaffected.
console.log(`PLAIN body:${await (await fetch(`${base}/plain`)).text()}`);

// Hang up while the handler is waiting.
const started = performance.now();
const client = new AbortController();
const inflight = fetch(`${base}/slow`, { signal: client.signal }).catch(() => "hung up");
await new Promise((r) => setTimeout(r, 150));
client.abort();
await inflight;

// Give the disconnect a moment to cross back to the handler.
await new Promise((r) => setTimeout(r, 300));
const elapsed = performance.now() - started;
console.log(`ABORT reason:${seen.abortedReason} promptly:${elapsed < 3000}`);

// A request that completes normally must not look like a disconnect.
console.log(`QUICK body:${await (await fetch(`${base}/quick`)).text()}`);
await new Promise((r) => setTimeout(r, 200));
console.log(`QUICK aborted:${seen.completedSignal.aborted}`);

await server.stop();
// Reaching here at all means no disconnect watch was left pending: an op that
// never settled would hold the loop open and this process would not exit.
console.log("SIGNAL_OK");
