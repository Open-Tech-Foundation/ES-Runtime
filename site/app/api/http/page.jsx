import ApiShell from "../../../components/ApiShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const SERVE = `import { serve } from "runtime:http";

// The handler takes a web Request and returns a web Response —
// the same Fetch API objects fetch() uses.
const server = serve({ port: 8080 }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/echo") {
    return new Response(await request.text(), { status: 200 });
  }
  return Response.json({ method: request.method, path: url.pathname });
});

const { hostname, port } = await server.addr;
console.log(\`listening on http://\${hostname}:\${port}\`);`;

const STOP = `const server = serve(handler);          // ephemeral port
const { port } = await server.addr;     // resolved once listening
// … handle requests …
await server.stop();                    // stop accepting; finished resolves`;

const fns = [
  {
    sig: "serve(handler)",
    type: "(Handler) => Server",
    desc: "Start a server on an ephemeral port (read it from server.addr). handler is (request) => response.",
    ex: `const server = serve((req) => new Response("hi"));`,
  },
  {
    sig: "serve(options, handler)",
    type: "({ hostname?, port? }, Handler) => Server",
    desc: "Start a server bound to options. hostname defaults to 0.0.0.0; port 0 picks an ephemeral port.",
    ex: `serve({ hostname: "127.0.0.1", port: 8080 }, handler);`,
  },
];

const serverMembers = [
  { m: "addr", t: "Promise<{ hostname, port }>", d: "The bound address; resolves once the server is listening." },
  { m: "finished", t: "Promise<void>", d: "Resolves when the accept loop has ended (after stop())." },
  { m: "stop()", t: "Promise<void>", d: "Stop accepting and shut down; resolves once stopped." },
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

export default function HttpDoc() {
  return (
    <ApiShell active="/api/http">
      <p className="text-sm font-medium text-brand-600">API reference</p>
      <h1 className="mt-2 font-mono text-3xl font-bold tracking-tight text-zinc-900 sm:text-4xl">
        runtime:http
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        An HTTP/1.1 server: <code className="font-mono">serve((request) =&gt; response)</code>.
        The handler takes a web <code className="font-mono">Request</code> and returns
        a web <code className="font-mono">Response</code> — the same Fetch API objects{" "}
        <code className="font-mono">fetch</code> uses.
      </p>
      <div className="mt-5 flex flex-wrap items-center gap-2 text-xs">
        <span className="rounded-full bg-brand-50 px-3 py-1 font-medium text-brand-700">
          Capability: NetListen
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
          <strong>All I/O is async.</strong> A handler error or a non-
          <code className="font-mono">Response</code> return becomes a{" "}
          <code className="font-mono">500</code>. Request and response bodies are
          buffered. TLS is not supported yet — terminate it at a proxy.
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

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Serve</h2>
      <div className="mt-4">
        <CodeBlock code={SERVE} title="server.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Lifecycle</h2>
      <p className="mt-3 text-sm leading-relaxed text-zinc-600">
        <code className="font-mono">serve()</code> returns a{" "}
        <code className="font-mono">Server</code> immediately; the accept loop runs
        in the background.
      </p>
      <div className="mt-4">
        <CodeBlock code={STOP} title="lifecycle.js" lang="js" />
      </div>
      <h3 className="mt-8 text-base font-semibold text-zinc-900">Server</h3>
      <MemberTable rows={serverMembers} />
    </ApiShell>
  );
}
