import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import MemberTable from "../../../components/MemberTable.jsx";
import ErrorTable from "../../../components/ErrorTable.jsx";

const errors = [
  { e: "SyntaxError", w: "new WebSocket(url) with an invalid URL or scheme, or close(code, reason) with a reason longer than 123 UTF-8 bytes." },
  { e: "DOMException", w: "name \"InvalidAccessError\" — close(code) with a code other than 1000 or 3000–4999." },
  { e: "DOMException", w: "name \"InvalidStateError\" — send() while the socket is still CONNECTING." },
  { e: "(event)", w: "A failed connection or a denied Net capability surfaces as an error event followed by a close (code 1006) — not a thrown exception." },
];

const CLIENT = `// The WebSocket global — like fetch, no import. Opening requires Net.
const ws = new WebSocket("wss://example.com/socket", ["chat"]);
ws.binaryType = "arraybuffer"; // or "blob" (default)

ws.addEventListener("open", () => ws.send("hello"));
ws.addEventListener("message", (e) => {
  // e.data: string (text) | ArrayBuffer | Blob (binary, per binaryType)
  console.log(e.data, e.origin);
});
ws.addEventListener("close", (e) => console.log(e.code, e.reason, e.wasClean));

ws.close(1000, "done"); // code 1000 or 3000–4999; reason ≤ 123 UTF-8 bytes`;

const SERVER = `import { serve, broadcast } from "runtime:websocket";

const clients = new Set();
const server = serve({ hostname: "127.0.0.1", port: 4001 });
const { port } = await server.addr;

for await (const ws of server) {
  clients.add(ws);
  // broadcast() fans out in one host crossing — full delivery, coalesced writes.
  ws.addEventListener("message", (e) => broadcast(clients, e.data));
  ws.addEventListener("close", () => clients.delete(ws));
}`;

const clientMembers = [
  { m: "new WebSocket(url, protocols?)", t: "(url, string | string[]) => WebSocket", d: "url must be ws:/wss: with no fragment; protocols are RFC 6455 tokens. Requires Net." },
  { m: "readyState", t: "0 | 1 | 2 | 3", d: "CONNECTING / OPEN / CLOSING / CLOSED — constants on the instance and the interface." },
  { m: "send(data)", t: "(BufferSource | Blob | USVString) => void", d: "Throws InvalidStateError while CONNECTING; dropped silently after close." },
  { m: "close(code?, reason?)", t: "(number?, string?) => void", d: "code = 1000 or 3000–4999 (else InvalidAccessError); reason ≤ 123 UTF-8 bytes (else SyntaxError)." },
  { m: "binaryType", t: '"blob" | "arraybuffer"', d: "How binary messages surface in message events (default \"blob\")." },
  { m: "bufferedAmount", t: "number", d: "Best-effort bytes queued by send but not yet flushed." },
  { m: "protocol / extensions / url", t: "string", d: "Negotiated subprotocol / extensions (\"\" — none) / the resolved URL." },
  { m: "on{open,message,error,close}", t: "EventHandler", d: "Also via addEventListener. message → MessageEvent; close → CloseEvent." },
];

const serveExports = [
  { m: "serve(options)", t: "({ hostname?, port }) => WebSocketServer", d: "Bind a WebSocket server (ws: only). port 0 picks an ephemeral port (read it from server.addr). NetListen." },
  { m: "broadcast(connections, data)", t: "(Iterable<conn>, string | BufferSource | Blob) => void", d: "Send one message to many connections in a single host crossing — the batched form of a .send() loop (concurrent enqueue, coalesced writes, full delivery)." },
];

const serverMembers = [
  { m: "addr", t: "Promise<{ hostname, port }>", d: "The bound address (resolves once listening)." },
  { m: "accept()", t: "Promise<connection | null>", d: "The next connection, or null once closed." },
  { m: "close()", t: "Promise<void>", d: "Stop accepting new connections." },
  { m: "[Symbol.asyncIterator]", t: "AsyncIterable<connection>", d: "for await (const ws of server) { … }" },
];

const connMembers = [
  { m: "send(data)", t: "(string | Blob | BufferSource) => void", d: "Send a text or binary frame." },
  { m: "close(code?, reason?)", t: "(number?, string?) => void", d: "Begin the closing handshake." },
  { m: "binaryType", t: '"blob" | "arraybuffer"', d: "How binary messages surface (default \"blob\")." },
  { m: "on{message,close,error}", t: "EventHandler", d: "Also via addEventListener — the client surface, minus the connecting handshake." },
];

export default function WebSocketDoc() {
  return (
    <ApiShell active="/api/websocket">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        WebSocket
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The classic WHATWG <code className="font-mono">WebSocket</code> — a{" "}
        <strong>client</strong> as a global (like{" "}
        <code className="font-mono">fetch</code>) and a <strong>server</strong> as{" "}
        <code className="font-mono">runtime:websocket</code>. Push-based{" "}
        <code className="font-mono">message</code>/<code className="font-mono">close</code>{" "}
        events ride the runtime's tick; <code className="font-mono">wss:</code>{" "}
        reuses the same TLS stack as <code className="font-mono">runtime:net</code>.
      </p>

      {/* ---- Client ---- */}
      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">Client</h2>
      <p className="mt-2 text-sm leading-relaxed text-zinc-600">
        The <code className="font-mono">WebSocket</code> global — no import.
      </p>
      <div className="mt-4 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: Net
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          Global · ws: / wss:
        </span>
      </div>
      <div className="mt-4">
        <CodeBlock code={CLIENT} title="client.js" lang="js" />
      </div>
      <div className="mt-6 flex items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 p-4 text-sm leading-relaxed text-amber-900">
        <span className="mt-0.5 shrink-0 font-semibold">Net</span>
        <span>
          Without the <strong>Net</strong> capability (or with no WebSocket
          provider installed) the socket fails with an{" "}
          <code className="font-mono">error</code> then a{" "}
          <code className="font-mono">close</code> (code 1006).
        </span>
      </div>
      <h3 className="mt-8 text-base font-semibold text-zinc-900">Interface</h3>
      <MemberTable rows={clientMembers} />

      {/* ---- Server ---- */}
      <h2 id="server" className="mt-16 text-2xl font-semibold text-zinc-900">
        Server · <span className="font-mono text-xl">runtime:websocket</span>
      </h2>
      <p className="mt-2 text-sm leading-relaxed text-zinc-600">
        Serving is capability-gated I/O, so it lives in a{" "}
        <code className="font-mono">runtime:</code> module like{" "}
        <code className="font-mono">runtime:net</code>{" "}
        <code className="font-mono">listen()</code>.{" "}
        <code className="font-mono">serve()</code> yields accepted connections —
        each the same shape as a client socket, already open.
      </p>
      <div className="mt-4 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: NetListen
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · ws:
        </span>
      </div>
      <div className="mt-4">
        <CodeBlock code={SERVER} title="chat.js" lang="js" />
      </div>
      <h3 className="mt-8 text-base font-semibold text-zinc-900">Exports</h3>
      <MemberTable rows={serveExports} />
      <h3 className="mt-8 text-base font-semibold text-zinc-900">WebSocketServer</h3>
      <MemberTable rows={serverMembers} />
      <h3 className="mt-8 text-base font-semibold text-zinc-900">connection</h3>
      <MemberTable rows={connMembers} />

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Errors</h2>
      <ErrorTable rows={errors} />

      <p className="mt-8 text-sm leading-relaxed text-zinc-500">
        Not yet: the promise/stream-based{" "}
        <code className="font-mono">WebSocketStream</code>, permessage-deflate
        (<code className="font-mono">extensions</code> is always{" "}
        <code className="font-mono">""</code>), a <code className="font-mono">wss:</code>{" "}
        server, and pub/sub topics over the explicit-set{" "}
        <code className="font-mono">broadcast()</code>.
      </p>
    </ApiShell>
  );
}
