// The server half of the HTTP soak: plain, echoed and streamed responses over
// runtime:http, and a WebSocket echo over runtime:websocket. It keeps no state
// between requests, so anything that grows while it runs is the runtime's.
import { serve } from "runtime:http";
import { serve as serveWs } from "runtime:websocket";
import { env } from "runtime:process";

const PORT = Number(env.SOAK_PORT);
const CHUNK = new Uint8Array(16 * 1024).fill(97);

serve({ hostname: "127.0.0.1", port: PORT }, async (req) => {
  const { pathname } = new URL(req.url);
  if (pathname === "/echo") return Response.json(await req.json());
  if (pathname === "/stream") {
    let left = 8;
    return new Response(
      new ReadableStream({
        pull(controller) {
          if (left-- === 0) controller.close();
          else controller.enqueue(CHUNK);
        },
      }),
    );
  }
  return new Response("ok");
});

const ws = serveWs({ hostname: "127.0.0.1", port: PORT + 1 });
for await (const socket of ws) {
  socket.onmessage = (e) => socket.send(e.data);
}
