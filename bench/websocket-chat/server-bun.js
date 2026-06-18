// Broadcast "chat" server on Bun (Bun.serve WebSocket). Every received message
// is delivered to all connected clients — the chat fan-out the benchmark times.
const PORT = Number(globalThis.__WS_PORT || 4001);
const clients = new Set();

Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  fetch(req, server) {
    if (server.upgrade(req)) return;
    return new Response("", { status: 400 });
  },
  websocket: {
    open(ws) {
      clients.add(ws);
    },
    close(ws) {
      clients.delete(ws);
    },
    message(_ws, msg) {
      for (const c of clients) c.send(msg);
    },
  },
});
