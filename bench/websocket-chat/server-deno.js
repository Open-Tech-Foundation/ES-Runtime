// Broadcast "chat" server on Deno (Deno.upgradeWebSocket). Every received
// message is delivered to all connected clients — the chat fan-out timed here.
const PORT = Number(globalThis.__WS_PORT || 4001);
const clients = new Set();

Deno.serve({ port: PORT, hostname: "127.0.0.1" }, (req) => {
  const { socket, response } = Deno.upgradeWebSocket(req);
  socket.onopen = () => clients.add(socket);
  socket.onclose = () => clients.delete(socket);
  socket.onmessage = (e) => {
    for (const c of clients) {
      try {
        c.send(e.data);
      } catch {}
    }
  };
  return response;
});
