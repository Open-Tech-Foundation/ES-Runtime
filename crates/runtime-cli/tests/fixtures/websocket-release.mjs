// A WebSocket's id leaves its agent's table when its stream ends (D183), and a
// broadcast that still names a connection which closed reaches every other.
import { serve } from "runtime:http";
import { upgradeWebSocket, broadcast } from "runtime:websocket";
import { inventory } from "runtime:diagnostics";

const sockets = [];
const ended = [];
const server = serve({ port: 0, hostname: "127.0.0.1" }, (request) => {
  const { response, socket } = upgradeWebSocket(request);
  sockets.push(socket);
  socket.onclose = () => ended.push(socket);
  return response;
});
const { port } = await server.addr;
const count = () => inventory().handles.find((h) => h.kind === "WebSocket")?.count ?? 0;

const open = (name) =>
  new Promise((resolve) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}/${name}`);
    ws.onopen = () => resolve(ws);
  });
const a = await open("a");
const b = await open("b");
while (sockets.length < 2) await new Promise((r) => setTimeout(r, 5));
// Both ends of both connections live in this agent.
console.log("open", count());

const closedOnServer = new Promise((r) => (sockets[0].onclose = () => r()));
await new Promise((resolve) => {
  a.onclose = resolve;
  a.close(1000, "bye");
});
await closedOnServer;
console.log("after-close", count());

// The closed connection is still in the list it is broadcast to.
const got = new Promise((r) => (b.onmessage = (e) => r(e.data)));
broadcast(sockets, "still-here");
console.log("broadcast", await got);

// Calls on the ended sockets are quiet, as the spec has them.
a.send("late");
a.close();
sockets[0].send("late");
sockets[0].close();
await new Promise((r) => setTimeout(r, 20));

b.close();
await new Promise((r) => setTimeout(r, 50));
console.log("all-closed", count());
await server.stop();
