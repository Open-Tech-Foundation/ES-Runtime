// runtime:net — TCP sockets. Run with:
//   esrun examples/modules/net.mjs
// connect() follows the WinterTC Sockets API; listen() yields incoming Sockets.
import { connect, listen } from "runtime:net";

// A one-shot echo server.
const server = listen({ hostname: "127.0.0.1", port: 0 });
const { port } = await server.addr;
console.log("listening on", port);

(async () => {
  for await (const conn of server) {
    const writer = conn.writable.getWriter();
    for await (const chunk of conn.readable) await writer.write(chunk);
    await writer.close();
    await server.close();
  }
})();

// Client: connect, send, read the echo back.
const sock = connect({ hostname: "127.0.0.1", port });
const writer = sock.writable.getWriter();
await writer.write(new TextEncoder().encode("hello"));
await writer.close();

let reply = "";
const dec = new TextDecoder();
for await (const chunk of sock.readable) reply += dec.decode(chunk);
console.log("echo:", reply);
