// Connection timeouts end to end, over real sockets: a client that stalls is
// disconnected, a client that is working is not, and the option that turns each
// one off is honoured. Only the driven CLI can show this — it needs a peer that
// actually goes quiet on a socket, which `fetch` will never do for us.
import { serve } from "runtime:http";
import { connect } from "runtime:net";

// Short enough for a test to wait out; long enough that a busy machine does not
// trip them on a connection that is genuinely progressing.
const SHORT = 300;

// Opens a raw socket, optionally sends `greeting`, then reads until the server
// hangs up. Resolves `true` if it did within `grace`, `false` if the connection
// was still open. A server may answer before closing (hyper sends a 408 on a
// header timeout), so bytes are read past rather than taken as an answer.
async function closedWithin(port, greeting, grace) {
  const socket = await connect({ hostname: "127.0.0.1", port });
  if (greeting) {
    const writer = socket.writable.getWriter();
    await writer.write(new TextEncoder().encode(greeting));
    writer.releaseLock();
  }
  const reader = socket.readable.getReader();
  const closed = (async () => {
    try {
      for (;;) {
        const { done } = await reader.read();
        if (done) return true;
      }
    } catch {
      return true; // a reset is a close too
    }
  })();
  const timer = new Promise((resolve) => setTimeout(() => resolve(false), grace));
  const result = await Promise.race([closed, timer]);
  try {
    await socket.close();
  } catch {
    // Already gone, which is the case this test is usually in.
  }
  return result;
}

const guarded = serve(
  { port: 0, timeouts: { handshake: SHORT, headerRead: SHORT, h2KeepAlive: SHORT } },
  () => new Response("guarded"),
);
const guardedPort = (await guarded.addr).port;

// Connect and say nothing at all: the cheapest way to hold a descriptor.
console.log(`silent-closed:${await closedWithin(guardedPort, null, SHORT * 20)}`);

// A request head that starts and never ends — slowloris.
const partialHead = "GET / HTTP/1.1\r\nHost: x\r\n";
console.log(`dribble-closed:${await closedWithin(guardedPort, partialHead, SHORT * 20)}`);

// A real request on the same server still works, and is answered normally.
const ok = await fetch(`http://127.0.0.1:${guardedPort}/`);
console.log(`request-ok:${ok.status}:${await ok.text()}`);

await guarded.stop();

// The same stall against a server that has turned the timeouts off stays open:
// a deployment behind a proxy that already does this can opt out.
const open = serve(
  { port: 0, timeouts: { handshake: null, headerRead: null, h2KeepAlive: null } },
  () => new Response("open"),
);
const openPort = (await open.addr).port;
console.log(`disabled-closed:${await closedWithin(openPort, null, SHORT * 4)}`);
await open.stop();

// A bad value is rejected before anything binds.
let rejected = "none";
try {
  serve({ port: 0, timeouts: { headerRead: "soon" } }, () => new Response("x"));
} catch (e) {
  rejected = e.constructor.name;
}
console.log(`bad-option:${rejected}`);

console.log("TIMEOUTS_OK");
