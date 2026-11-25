// The server half of the HTTP soak: plain, echoed and streamed responses over
// runtime:http, and a WebSocket echo over runtime:websocket. It keeps no state
// between requests, so anything that grows while it runs is the runtime's. The
// same handler also listens with TLS on PORT + 2, where ALPN picks HTTP/2.
import { file } from "runtime:fs";
import { serve } from "runtime:http";
import { serve as serveWs } from "runtime:websocket";
import { env } from "runtime:process";

const PORT = Number(env.SOAK_PORT);
const CHUNK = new Uint8Array(16 * 1024).fill(97);

async function handler(req) {
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
}

serve({ hostname: "127.0.0.1", port: PORT }, handler);
serve(
  {
    hostname: "127.0.0.1",
    port: PORT + 2,
    secureTransport: "on",
    cert: await file("tls-cert.pem").text(),
    key: await file("tls-key.pem").text(),
  },
  handler,
);

const ws = serveWs({ hostname: "127.0.0.1", port: PORT + 1 });
for await (const socket of ws) {
  socket.onmessage = (e) => socket.send(e.data);
}
