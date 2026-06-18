// Broadcast "chat" server on esrun (runtime:websocket). Every received message
// is delivered to all connected clients — the chat fan-out timed here, via the
// batched `broadcast()` (one host crossing for the whole room).
import { serve, broadcast } from "runtime:websocket";

const PORT = Number(globalThis.__WS_PORT || 4001);
const clients = new Set();

const server = serve({ port: PORT, hostname: "127.0.0.1" });
for await (const ws of server) {
  clients.add(ws);
  ws.onmessage = (e) => broadcast(clients, e.data);
  ws.onclose = () => clients.delete(ws);
}
