import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const CLIENT = `import { connect } from "runtime:net";

// WinterTC connect() — returns a Socket immediately; .opened settles on connect.
const sock = connect({ hostname: "example.com", port: 80 });
await sock.opened;

const writer = sock.writable.getWriter();
await writer.write(new TextEncoder().encode("GET / HTTP/1.0\\r\\n\\r\\n"));
await writer.close();

let body = ""; const dec = new TextDecoder();
for await (const chunk of sock.readable) body += dec.decode(chunk);`;

const SERVER = `import { listen } from "runtime:net";

const server = listen({ hostname: "127.0.0.1", port: 8080 });
const { port } = await server.addr;

for await (const conn of server) {
  conn.readable.pipeTo(conn.writable); // echo each connection
}`;

const fns = [
  {
    sig: "connect(address, options?)",
    type: "(Address, { secureTransport? }) => Socket",
    desc: "Open an outbound TCP connection (the WinterTC Sockets API). Returns a Socket synchronously; .opened settles once connected. address is \"host:port\" or { hostname, port }.",
    ex: `const sock = connect({ hostname: "db.internal", port: 5432 });`,
  },
  {
    sig: "listen(options)",
    type: "({ hostname?, port }) => Listener",
    desc: "Bind a listening socket. port 0 picks an ephemeral port (read it from listener.addr). Returns an async-iterable Listener of inbound Sockets.",
    ex: `const server = listen({ hostname: "127.0.0.1", port: 8080 });`,
  },
];

const socketMembers = [
  { m: "readable", t: "ReadableStream<Uint8Array>", d: "Incoming bytes." },
  { m: "writable", t: "WritableStream<Uint8Array>", d: "Outgoing bytes; closing the writer half-closes (FIN)." },
  { m: "opened", t: "Promise<SocketInfo>", d: "Resolves once connected, with the address info." },
  { m: "closed", t: "Promise<void>", d: "Resolves when the socket is fully closed." },
  { m: "close()", t: "Promise<void>", d: "Fully close the socket." },
];

const listenerMembers = [
  { m: "addr", t: "Promise<{ hostname, port }>", d: "The bound address (resolves after bind)." },
  { m: "accept()", t: "Promise<Socket | null>", d: "The next connection, or null once closed." },
  { m: "close()", t: "Promise<void>", d: "Stop listening." },
  { m: "[Symbol.asyncIterator]", t: "AsyncIterable<Socket>", d: "for await (const conn of server) { … }" },
];

function MemberTable({ rows }) {
  return (
    <div className="mt-4 overflow-hidden rounded-xl border border-zinc-200">
      <table className="w-full text-left text-sm">
        <thead className="bg-zinc-50 text-xs uppercase tracking-wider text-zinc-500">
          <tr>
            <th className="px-4 py-3 font-semibold">Member</th>
            <th className="px-4 py-3 font-semibold">Type</th>
            <th className="px-4 py-3 font-semibold">Description</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-100">
          {rows.map((x) => (
            <tr>
              <td className="px-4 py-3 font-mono text-[13px] font-medium text-zinc-900">{x.m}</td>
              <td className="px-4 py-3 font-mono text-[13px] text-zinc-500">{x.t}</td>
              <td className="px-4 py-3 text-zinc-600">{x.d}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default function NetDoc() {
  return (
    <ApiShell active="/api/net">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:net
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        TCP sockets. <code className="font-mono">connect()</code> follows the
        WinterTC Sockets API; <code className="font-mono">listen()</code> yields
        inbound connections. Bytes move over web streams.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: Net (connect) / NetListen (listen)
        </span>
        <span className="rounded-full bg-zinc-100 px-3 py-1 font-medium text-zinc-600">
          ES module · runtime: scheme
        </span>
        <span className="rounded-full bg-emerald-50 px-3 py-1 font-medium text-emerald-700">
          Available
        </span>
      </div>

      <div className="mt-6 flex items-start gap-3 rounded-xl border border-amber-200 bg-amber-50 p-4 text-sm leading-relaxed text-amber-900">
        <span className="mt-0.5 shrink-0 font-semibold">async</span>
        <span>
          <strong>All I/O is async</strong> over web streams — nothing blocks the
          event loop. Closing a socket's <code className="font-mono">writable</code>{" "}
          half-closes (sends FIN) while reads continue. TLS is not supported yet.
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Client</h2>
      <div className="mt-4">
        <CodeBlock code={CLIENT} title="client.js" lang="js" />
      </div>
      <h3 className="mt-8 text-base font-semibold text-zinc-900">Socket</h3>
      <MemberTable rows={socketMembers} />

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Server</h2>
      <div className="mt-4">
        <CodeBlock code={SERVER} title="server.js" lang="js" />
      </div>
      <h3 className="mt-8 text-base font-semibold text-zinc-900">Listener</h3>
      <MemberTable rows={listenerMembers} />
    </ApiShell>
  );
}
