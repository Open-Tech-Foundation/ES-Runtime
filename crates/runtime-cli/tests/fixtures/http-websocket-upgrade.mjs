// One port, both protocols — the shape Node, Deno and Bun all settle on.
import { serve } from "runtime:http";
import { upgradeWebSocket, broadcast } from "runtime:websocket";

const room = new Set();
const server = serve({ port: 0, hostname: "127.0.0.1" }, (request) => {
  // A request that never asked to be upgraded: refused before any 101.
  if (new URL(request.url).pathname === "/not-a-websocket") {
    try {
      upgradeWebSocket(request);
      return new Response("NO THROW not-a-websocket");
    } catch (e) {
      return new Response(`not-upgradable ${e.constructor.name}`);
    }
  }
  if (request.headers.get("upgrade") === "websocket") {
    // Answer the client's second choice when it offers a list, and refuse one
    // it never offered.
    const offered = request.headers.get("sec-websocket-protocol") ?? "";
    if (offered.includes("chat.v2")) {
      try {
        upgradeWebSocket(request, { protocol: "chat.v9" });
        console.log("NO THROW protocol");
      } catch {
        console.log("refused-protocol");
      }
    }
    const { response, socket } = upgradeWebSocket(
      request,
      offered.includes("chat.v2") ? { protocol: "chat.v2" } : {},
    );
    room.add(socket);
    socket.onmessage = (e) => broadcast(room, `room:${e.data}`);
    socket.onclose = () => room.delete(socket);
    return response;
  }
  return new Response(`api:${new URL(request.url).pathname}`);
});
const { port } = await server.addr;

// The same port answers an ordinary request.
const res = await fetch(`http://127.0.0.1:${port}/hello`);
console.log("http", res.status, await res.text());

// An ordinary request cannot be made a WebSocket: a TypeError, not a 101.
const plain = await fetch(`http://127.0.0.1:${port}/not-a-websocket`);
console.log("plain", plain.status, await plain.text());

// …and upgrades one that asks.
const a = new WebSocket(`ws://127.0.0.1:${port}/socket`);
await new Promise((r) => (a.onopen = r));
const seen = new Promise((r) => (a.onmessage = (e) => r(e.data)));
a.send("one");
console.log("ws", await seen);

// An upgraded socket is a connection like any other: broadcast reaches it.
const b = new WebSocket(`ws://127.0.0.1:${port}/socket`);
await new Promise((r) => (b.onopen = r));
const both = Promise.all([
  new Promise((r) => (a.onmessage = (e) => r(e.data))),
  new Promise((r) => (b.onmessage = (e) => r(e.data))),
]);
b.send("two");
console.log("broadcast", (await both).join("|"));

// A Request that did not come from a handler has no connection to take over.
try {
  upgradeWebSocket(new Request("http://x/"));
  console.log("NO THROW");
} catch (e) {
  console.log("refused", e.constructor.name);
}

// Subprotocol negotiation, both directions.
const c = new WebSocket(`ws://127.0.0.1:${port}/socket`, ["chat.v1", "chat.v2"]);
await new Promise((r) => (c.onopen = r));
console.log("protocol", c.protocol);
c.close();

a.close();
b.close();
await server.stop();
