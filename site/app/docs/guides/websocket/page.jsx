import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

const CLIENT = `// WebSocket is a global — like fetch, no import. Opening requires Net.
const ws = new WebSocket("wss://example.com/feed", ["json"]);

ws.addEventListener("open", () => ws.send(JSON.stringify({ hello: "world" })));
ws.addEventListener("message", (e) => {
  const msg = JSON.parse(e.data);          // e.data is a string for text frames
  console.log("got", msg);
});
ws.addEventListener("close", (e) => console.log("closed", e.code, e.reason));
ws.addEventListener("error", () => console.log("connection failed"));`;

const BINARY = `const ws = new WebSocket("ws://localhost:9001/");
ws.binaryType = "arraybuffer";             // default is "blob"

ws.addEventListener("open", () => ws.send(new Uint8Array([1, 2, 3]).buffer));
ws.addEventListener("message", (e) => {
  if (typeof e.data === "string") console.log("text", e.data);
  else console.log("binary", new Uint8Array(e.data)); // ArrayBuffer
});`;

const CLOSE = `ws.close();                  // normal (1000)
ws.close(1000, "bye");       // code 1000 or 3000–4999; reason ≤ 123 UTF-8 bytes
// send() while CONNECTING throws InvalidStateError; after close it is ignored.`;

const SERVER = `import { serve } from "runtime:websocket";

const server = serve({ hostname: "127.0.0.1", port: 9001 });
const { port } = await server.addr;        // resolves once listening
console.log("listening on", port);

for await (const ws of server) {           // each accepted connection
  ws.addEventListener("message", (e) => ws.send(e.data)); // echo
}`;

const CHAT = `import { serve, broadcast } from "runtime:websocket";

const clients = new Set();
const server = serve({ hostname: "127.0.0.1", port: 9001 });

for await (const ws of server) {
  clients.add(ws);
  // broadcast() sends to the whole room in ONE host crossing — far cheaper than
  // a c.send() loop, with full delivery and coalesced socket writes.
  ws.addEventListener("message", (e) => broadcast(clients, e.data));
  ws.addEventListener("close", () => clients.delete(ws));
}`;

export default function WebSocketGuide() {
  return (
    <DocsShell active="/docs/guides/websocket">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        WebSockets
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        esrun ships the classic WHATWG{" "}
        <a href="/api/websocket" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">WebSocket</code>
        </a>{" "}
        on both sides: a <strong>client</strong> as a global (like{" "}
        <code className="font-mono">fetch</code>) and a <strong>server</strong> as
        the <code className="font-mono">runtime:websocket</code> module. Both
        speak <code className="font-mono">ws:</code> and{" "}
        <code className="font-mono">wss:</code> (client); messages ride the
        runtime's event loop — no extra threads.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">Client</h2>
      <p className="mt-3 text-zinc-600">
        Construct a <code className="font-mono">WebSocket</code> and listen for{" "}
        <code className="font-mono">open</code>,{" "}
        <code className="font-mono">message</code>,{" "}
        <code className="font-mono">close</code>, and{" "}
        <code className="font-mono">error</code>. Opening a connection needs the{" "}
        <strong>Net</strong> capability (the esrun CLI grants it).
      </p>
      <div className="mt-5">
        <CodeBlock code={CLIENT} title="client.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Binary data</h3>
      <p className="mt-3 text-zinc-600">
        Send strings, <code className="font-mono">ArrayBuffer</code>,{" "}
        typed arrays, or <code className="font-mono">Blob</code>. Set{" "}
        <code className="font-mono">binaryType</code> to choose how binary frames
        arrive.
      </p>
      <div className="mt-5">
        <CodeBlock code={BINARY} title="binary.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Closing</h3>
      <div className="mt-5">
        <CodeBlock code={CLOSE} title="close.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">Server</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">serve()</code> binds a server (needs{" "}
        <strong>NetListen</strong>) and yields each accepted connection as an
        async iterable — every connection has the same{" "}
        <code className="font-mono">send</code>/<code className="font-mono">close</code>{" "}
        and <code className="font-mono">message</code>/<code className="font-mono">close</code>{" "}
        surface as a client socket, already open.
      </p>
      <div className="mt-5">
        <CodeBlock code={SERVER} title="echo-server.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">Broadcast (chat)</h3>
      <p className="mt-3 text-zinc-600">
        For chat-style fan-out, keep a set of connections and use{" "}
        <code className="font-mono">broadcast()</code> — it delivers one message
        to the whole room in a single host crossing with one payload copy, so it
        stays fast and lossless where a per-connection{" "}
        <code className="font-mono">.send()</code> loop would lag under load.
      </p>
      <div className="mt-5">
        <CodeBlock code={CHAT} title="chat-server.js" lang="js" />
      </div>
      <p className="mt-4 text-sm text-zinc-500">
        See the{" "}
        <a href="/docs/benchmarks#websocket" className="font-medium text-brand-600 hover:text-brand-700">
          WebSocket benchmarks
        </a>{" "}
        for fan-out throughput. A <code className="font-mono">wss:</code> server
        and pub/sub topics are on the roadmap.
      </p>
    </DocsShell>
  );
}
