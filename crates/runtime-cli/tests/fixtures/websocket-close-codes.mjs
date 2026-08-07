// The close code an initiator reports to itself, against a real server.
//
// A client calling `close(4001, "bye")` used to get code 1006 / wasClean false
// in its own close handler, while the peer correctly received 4001 / "bye".
// 1006 means "connection dropped without a close frame" and must never mark a
// clean shutdown — reconnect logic keyed on the code took the failure branch on
// every ordinary close.
import { serve } from "runtime:websocket";

const server = serve({ port: 0, hostname: "127.0.0.1" });
const { port } = await server.addr;
const url = `ws://127.0.0.1:${port}`;

// Echo the code the server side observed, so both ends are compared.
const seen = [];
(async () => {
  for await (const ws of server) {
    ws.addEventListener("close", (e) => seen.push(`${e.code}/${e.reason}/${e.wasClean}`));
    ws.addEventListener("message", (e) => {
      if (e.data === "close-me") ws.close(4002, "server-said-so");
    });
    ws.send("hello");
  }
})().catch(() => {});

const open = () =>
  new Promise((resolve) => {
    const w = new WebSocket(url);
    w.onopen = () => resolve(w);
  });
const first = (w) => new Promise((resolve) => { w.onmessage = () => resolve(); });

async function closeWith(label, apply) {
  const w = await open();
  await first(w);
  const ev = await new Promise((resolve) => {
    w.onclose = resolve;
    apply(w);
  });
  // Give the server's own close listener a turn before reading `seen`.
  await new Promise((r) => setTimeout(r, 100));
  console.log(`${label} client:${ev.code}/${ev.reason}/${ev.wasClean} server:${seen.pop() ?? "-"}`);
}

await closeWith("custom", (w) => w.close(4001, "bye"));
await closeWith("normal", (w) => w.close(1000));
await closeWith("nocode", (w) => w.close());

// A server-initiated close is reported to the client unchanged.
const w = await open();
await first(w);
const ev = await new Promise((resolve) => {
  w.onclose = resolve;
  w.send("close-me"); // the server closes with 4002
});
console.log(`server-initiated client:${ev.code}/${ev.reason}/${ev.wasClean}`);
await server.close();
console.log("WS_CLOSE_OK");
