// The connection cap over real sockets: with one slot, a second connection is
// held back until the first lets go — and it is *held*, not refused, so it is
// served rather than dropped once a slot frees. Only the driven CLI can show
// this; it needs a real accept loop with a real backlog behind it.
import { serve } from "runtime:http";
import { connect } from "runtime:net";

const server = serve({ port: 0, maxConnections: 1 }, () => new Response("served"));
const { port } = await server.addr;

// A raw socket, so the connection stays open (and keeps its slot) after the
// response — `fetch` would return the body and leave the pooling to chance.
async function openRequest() {
  const socket = await connect({ hostname: "127.0.0.1", port });
  const writer = socket.writable.getWriter();
  await writer.write(new TextEncoder().encode("GET / HTTP/1.1\r\nHost: x\r\n\r\n"));
  writer.releaseLock();
  return socket;
}

// Resolves with the first bytes the server sends, or "silent" if none arrive
// within `grace`.
async function firstReply(socket, grace) {
  const reader = socket.readable.getReader();
  const read = reader.read().then(({ value, done }) =>
    done ? "closed" : new TextDecoder().decode(value).split("\r\n")[0],
  );
  const timer = new Promise((resolve) => setTimeout(() => resolve("silent"), grace));
  const result = await Promise.race([read, timer]);
  reader.releaseLock();
  return result;
}

// The first connection takes the only slot and keeps it: an HTTP/1.1 keep-alive
// connection is still a live connection.
const first = await openRequest();
console.log(`first:${await firstReply(first, 5000)}`);

// The second connects — the kernel's backlog completes the handshake — but the
// server must not serve it while the slot is taken.
const second = await openRequest();
console.log(`second-while-full:${await firstReply(second, 1000)}`);

// Let the first go. Its slot frees, and the waiting connection is admitted:
// held back, never refused.
await first.close();
console.log(`second-after-free:${await firstReply(second, 10000)}`);

try {
  await second.close();
} catch {
  // Already gone.
}
await server.stop();
console.log("CAP_OK");
