// maxMessageSize and maxBufferedAmount on both server paths, through the real
// binary: a message under the limit is echoed, one over it closes with 1009,
// a bad value is refused before anything happens, and a single message larger
// than the send bound still goes out on an idle connection.
import { serve as http } from "runtime:http";
import { serve, upgradeWebSocket } from "runtime:websocket";

const echo = (socket) => {
  socket.binaryType = "arraybuffer";
  socket.onmessage = (e) => socket.send(e.data);
};

const server = http({ port: 0, hostname: "127.0.0.1" }, (request) => {
  const path = new URL(request.url).pathname;
  if (path === "/bad") {
    const refused = [];
    for (const options of [{ maxMessageSize: -1 }, { maxBufferedAmount: 1.5 }, { maxMessageSize: "1k" }]) {
      try {
        upgradeWebSocket(request, options);
        refused.push("NO THROW");
      } catch (e) {
        refused.push(e.constructor.name);
      }
    }
    // A refusal answers nothing: the request is still the guest's to decline.
    return new Response(refused.join(","));
  }
  const options = path === "/small" ? { maxMessageSize: 1024 } : {};
  const { response, socket } = upgradeWebSocket(request, options);
  echo(socket);
  return response;
});
const { port } = await server.addr;

const standalone = serve({ hostname: "127.0.0.1", port: 0, maxMessageSize: 1024 });
const { port: wsPort } = await standalone.addr;
(async () => {
  for await (const socket of standalone) echo(socket);
})();

// Sends `size` bytes; resolves with what came back, or how the socket closed.
const exchange = (url, size) =>
  new Promise((resolve) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";
    ws.onopen = () => ws.send(new Uint8Array(size));
    ws.onmessage = (e) => {
      resolve(`echo:${e.data.byteLength}`);
      ws.close();
    };
    ws.onclose = (e) => resolve(`closed:${e.code}`);
  });

console.log("bad", await (await fetch(`http://127.0.0.1:${port}/bad`, {
  headers: { upgrade: "websocket", connection: "upgrade" },
})).text());
console.log("upgrade-under", await exchange(`ws://127.0.0.1:${port}/small`, 1000));
console.log("upgrade-over", await exchange(`ws://127.0.0.1:${port}/small`, 2000));
console.log("serve-under", await exchange(`ws://127.0.0.1:${wsPort}/`, 1000));
console.log("serve-over", await exchange(`ws://127.0.0.1:${wsPort}/`, 2000));
// 20 MiB: past the 8 MiB send bound, under the 64 MiB message default.
console.log("upgrade-large", await exchange(`ws://127.0.0.1:${port}/`, 20 * 1024 * 1024));
await server.stop();
await standalone.close();
console.log("WS_LIMITS_OK");
