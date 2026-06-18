import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";
import MemberTable from "../../../components/MemberTable.jsx";

const CLIENT = `// A global, like fetch — no import. Opening requires the Net capability.
const ws = new WebSocket("wss://example.com/socket", ["chat"]);
ws.binaryType = "arraybuffer"; // or "blob" (default)

ws.addEventListener("open", () => ws.send("hello"));
ws.addEventListener("message", (e) => {
  // e.data: string (text) | ArrayBuffer | Blob (binary, per binaryType)
  console.log(e.data, e.origin);
});
ws.addEventListener("close", (e) => console.log(e.code, e.reason, e.wasClean));

ws.close(1000, "done"); // code 1000 or 3000–4999; reason ≤ 123 UTF-8 bytes`;

const members = [
  { m: "new WebSocket(url, protocols?)", t: "(url, string | string[]) => WebSocket", d: "url must be ws:/wss: with no fragment; protocols are RFC 6455 tokens. Requires the Net capability." },
  { m: "readyState", t: "0 | 1 | 2 | 3", d: "CONNECTING / OPEN / CLOSING / CLOSED — constants on the instance and the interface." },
  { m: "send(data)", t: "(BufferSource | Blob | USVString) => void", d: "Throws InvalidStateError while CONNECTING; dropped silently after close." },
  { m: "close(code?, reason?)", t: "(number?, string?) => void", d: "code = 1000 or 3000–4999 (else InvalidAccessError); reason ≤ 123 UTF-8 bytes (else SyntaxError)." },
  { m: "binaryType", t: '"blob" | "arraybuffer"', d: "How binary messages surface in message events (default \"blob\")." },
  { m: "bufferedAmount", t: "number", d: "Best-effort bytes queued by send but not yet flushed." },
  { m: "protocol / extensions / url", t: "string", d: "Negotiated subprotocol / extensions (\"\" — none) / the resolved URL." },
  { m: "on{open,message,error,close}", t: "EventHandler", d: "Also via addEventListener. message → MessageEvent; close → CloseEvent." },
];

export default function WebSocketDoc() {
  return (
    <ApiShell active="/api/websocket">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        WebSocket
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The classic WHATWG <code className="font-mono">WebSocket</code> interface
        — a global, like <code className="font-mono">fetch</code>. Push-based{" "}
        <code className="font-mono">message</code>/<code className="font-mono">close</code>{" "}
        events ride the runtime's tick; <code className="font-mono">wss:</code>{" "}
        reuses the same TLS stack as <code className="font-mono">runtime:net</code>.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: Net
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          Global · ws: / wss:
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Client</h2>
      <div className="mt-4">
        <CodeBlock code={CLIENT} title="socket.js" lang="js" />
      </div>

      <h3 className="mt-8 text-base font-semibold text-zinc-900">Interface</h3>
      <MemberTable rows={members} />

      <p className="mt-8 text-sm leading-relaxed text-zinc-500">
        Not yet: the promise/stream-based <code className="font-mono">WebSocketStream</code>,
        and permessage-deflate (<code className="font-mono">extensions</code> is always{" "}
        <code className="font-mono">""</code>).
      </p>
    </ApiShell>
  );
}
