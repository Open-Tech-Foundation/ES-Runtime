// Broadcast "chat" server on esrun (runtime:websocket `serve`). Every received
// message is delivered to all connected clients — the chat fan-out timed here.
import { serve } from "runtime:websocket";

const PORT = Number(globalThis.__WS_PORT || 4001);
const clients = new Set();

const server = serve({ port: PORT, hostname: "127.0.0.1" });
for await (const ws of server) {
  clients.add(ws);
  ws.onmessage = (e) => {
    const data = e.data;
    for (const c of clients) c.send(data);
  };
  ws.onclose = () => clients.delete(ws);
}
