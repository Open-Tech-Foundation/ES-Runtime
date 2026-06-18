import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import MemberTable from "../../../components/MemberTable.jsx";

const SERVE = `import { serve, broadcast } from "runtime:websocket";

const clients = new Set();
const server = serve({ hostname: "127.0.0.1", port: 4001 });
const { port } = await server.addr;

for await (const ws of server) {
  clients.add(ws);
  // broadcast() fans out in one host crossing — full delivery, coalesced writes.
  ws.addEventListener("message", (e) => broadcast(clients, e.data));
  ws.addEventListener("close", () => clients.delete(ws));
}`;

const fns = [
  {
    sig: "serve(options)",
    type: "({ hostname?, port }) => WebSocketServer",
    desc: "Bind a WebSocket server (ws: only) and start accepting. port 0 picks an ephemeral port (read it from server.addr). Returns an async-iterable of accepted connections. NetListen.",
    ex: `const server = serve({ hostname: "127.0.0.1", port: 4001 });`,
  },
  {
    sig: "broadcast(connections, data)",
    type: "(Iterable<conn>, string | BufferSource | Blob) => void",
    desc: "Send one message to many connections in a single host crossing — the batched form of a .send() loop (one payload copy, concurrent enqueue, coalesced writes, full delivery).",
    ex: `broadcast(clients, "hello everyone");`,
  },
];

const serverMembers = [
  { m: "addr", t: "Promise<{ hostname, port }>", d: "The bound address (resolves once listening)." },
  { m: "accept()", t: "Promise<connection | null>", d: "The next connection, or null once closed." },
  { m: "close()", t: "Promise<void>", d: "Stop accepting new connections." },
  { m: "[Symbol.asyncIterator]", t: "AsyncIterable<connection>", d: "for await (const ws of server) { … }" },
];

const connMembers = [
  { m: "send(data)", t: "(string | Blob | ArrayBuffer | ArrayBufferView) => void", d: "Send a text or binary frame." },
  { m: "close(code?, reason?)", t: "(number?, string?) => void", d: "Begin the closing handshake." },
  { m: "binaryType", t: '"blob" | "arraybuffer"', d: "How binary messages surface (default \"blob\")." },
  { m: "on{message,close,error}", t: "EventHandler", d: "Also via addEventListener. message → MessageEvent; close → CloseEvent." },
];

export default function WebSocketServerDoc() {
  return (
    <ApiShell active="/api/websocket-server">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:websocket
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The WebSocket <strong>server</strong> side. The client is the global{" "}
        <a href="/api/websocket" className="text-brand-600 hover:underline">
          <code className="font-mono">WebSocket</code>
        </a>
        ; serving is capability-gated I/O, so it lives here like{" "}
        <code className="font-mono">runtime:net</code>{" "}
        <code className="font-mono">listen()</code>.{" "}
        <code className="font-mono">serve()</code> yields accepted connections —
        each the same shape as a client socket, already open.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: NetListen
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · runtime: scheme · ws:
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Functions</h2>
      <div className="mt-5 space-y-4">
        {fns.map((e) => (
          <div className="rounded-xl border border-zinc-200 p-5">
            <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
              <code className="font-mono text-[15px] font-semibold text-zinc-900">{e.sig}</code>
              <code className="font-mono text-[13px] text-zinc-400">{e.type}</code>
            </div>
            <p className="mt-2 text-sm leading-relaxed text-zinc-600">{e.desc}</p>
            <code className="mt-3 block overflow-x-auto rounded-lg bg-zinc-950 px-3 py-2 font-mono text-[12px] text-emerald-300">
              {e.ex}
            </code>
          </div>
        ))}
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Broadcast chat</h2>
      <div className="mt-4">
        <CodeBlock code={SERVE} title="chat.js" lang="js" />
      </div>

      <h3 className="mt-8 text-base font-semibold text-zinc-900">WebSocketServer</h3>
      <MemberTable rows={serverMembers} />

      <h3 className="mt-8 text-base font-semibold text-zinc-900">connection</h3>
      <MemberTable rows={connMembers} />

      <p className="mt-8 text-sm leading-relaxed text-zinc-500">
        A <code className="font-mono">wss:</code> server and pub/sub topics are
        follow-ups — the explicit-connection-set{" "}
        <code className="font-mono">broadcast()</code> is the fan-out primitive.
      </p>
    </ApiShell>
  );
}
