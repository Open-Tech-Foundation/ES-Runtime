import DocsShell from "../../../../components/DocsShell.jsx";
import CodeBlock from "../../../../components/CodeBlock.jsx";

const CLIENT = `import { connect } from "runtime:net";

// connect() returns a Socket synchronously; .opened settles once connected.
const sock = connect({ hostname: "example.com", port: 80 });
await sock.opened;

const w = sock.writable.getWriter();
await w.write(new TextEncoder().encode("GET / HTTP/1.0\\r\\n\\r\\n"));
await w.close();                           // half-close: send FIN, keep reading

// Decode through TextDecoderStream so a multi-byte character split across two
// chunks is still decoded correctly.
let body = "";
for await (const chunk of sock.readable.pipeThrough(new TextDecoderStream())) {
  body += chunk;
}`;

const TLS = `import { connect } from "runtime:net";

// secureTransport: "on" — TLS with certificate verification, offering ALPN.
const sock = connect({ hostname: "example.com", port: 443 }, {
  secureTransport: "on",
  sni: "example.com",                      // optional; defaults to the host
  alpn: ["h2", "http/1.1"],
});
const { alpn } = await sock.opened;        // negotiated protocol, e.g. "h2" (or null)`;

const STARTTLS = `import { connect } from "runtime:net";

// "starttls" opens plaintext, then upgrades the SAME connection in place
// (the SMTP/IMAP/XMPP pattern).
const sock = connect({ hostname: "mail.example.com", port: 143 }, {
  secureTransport: "starttls",
});
// ... exchange the plaintext go-ahead, then:
const tls = sock.startTls();               // a new, encrypted Socket
console.log(tls.upgraded);                 // true`;

const SERVER = `import { listen } from "runtime:net";

const server = listen({ hostname: "127.0.0.1", port: 8080 });
const { port } = await server.addr;        // resolves once listening

for await (const conn of server) {         // each accepted Socket, already open
  conn.readable.pipeTo(conn.writable);     // echo
}`;

const SERVER_TLS = `import { listen } from "runtime:net";
import { file } from "runtime:fs";

// Terminate TLS on accept. cert/key are inline PEM (string or bytes), so the
// guest loads them itself — server TLS needs no capability beyond NetListen.
const server = listen({
  hostname: "127.0.0.1", port: 8443,
  secureTransport: "on",
  cert: await file("cert.pem").text(),     // PEM chain, leaf first
  key: await file("key.pem").text(),       // PKCS#8 / PKCS#1 / SEC1
  alpn: ["h2", "http/1.1"],
});

for await (const conn of server) {
  const { alpn } = await conn.opened;      // negotiated protocol
  conn.readable.pipeTo(conn.writable);     // every byte is encrypted
}`;

export default function NetworkingGuide() {
  return (
    <DocsShell active="/docs/guides/networking">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        Sockets
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        <code className="font-mono">runtime:net</code> is raw TCP following the{" "}
        <a
          href="https://sockets-api.proposal.wintertc.org/"
          className="font-medium text-brand-600 hover:text-brand-700"
        >
          WinterTC Sockets API
        </a>
        :{" "}
        <a href="/api/net" className="font-medium text-brand-600 hover:text-brand-700">
          <code className="font-mono">connect()</code>
        </a>{" "}
        for outbound connections,{" "}
        <code className="font-mono">listen()</code> for a server. Bytes move over
        web streams — nothing blocks.
      </p>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">Client</h2>
      <p className="mt-3 text-zinc-600">
        Opening an outbound connection needs the <strong>Net</strong> capability
        (the esrun CLI grants it). Closing the{" "}
        <code className="font-mono">writable</code> half-closes (sends FIN) while
        reads continue; pass <code className="font-mono">allowHalfOpen: true</code>{" "}
        to keep writing after the peer's FIN.
      </p>
      <div className="mt-5">
        <CodeBlock code={CLIENT} title="client.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">TLS</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">secureTransport: "on"</code> negotiates TLS
        with certificate verification on. <code className="font-mono">sni</code>{" "}
        sets the server name (used for both the SNI extension and hostname
        verification), and <code className="font-mono">alpn</code> offers
        protocols — the negotiated one comes back as{" "}
        <code className="font-mono">opened.alpn</code>.
      </p>
      <div className="mt-5">
        <CodeBlock code={TLS} title="tls.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">STARTTLS</h3>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">secureTransport: "starttls"</code> opens
        plaintext and upgrades the same connection in place via{" "}
        <code className="font-mono">startTls()</code>, which returns a new
        encrypted <code className="font-mono">Socket</code> (the original is
        consumed).
      </p>
      <div className="mt-5">
        <CodeBlock code={STARTTLS} title="starttls.js" lang="js" />
      </div>

      <h2 className="mt-12 text-2xl font-semibold text-zinc-900">Server</h2>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">listen()</code> binds a server (needs{" "}
        <strong>NetListen</strong>) and yields each accepted{" "}
        <code className="font-mono">Socket</code> as an async iterable.{" "}
        <code className="font-mono">port: 0</code> picks an ephemeral port — read
        it from <code className="font-mono">addr</code>.
      </p>
      <div className="mt-5">
        <CodeBlock code={SERVER} title="server.js" lang="js" />
      </div>

      <h3 className="mt-8 text-lg font-semibold text-zinc-900">TLS termination</h3>
      <p className="mt-3 text-zinc-600">
        <code className="font-mono">secureTransport: "on"</code> on the server
        terminates TLS on every accept — pass a PEM{" "}
        <code className="font-mono">cert</code> +{" "}
        <code className="font-mono">key</code> (and optional{" "}
        <code className="font-mono">alpn</code>). The cert/key are supplied
        inline, so server TLS needs no capability beyond the{" "}
        <strong>NetListen</strong> the bind already requires.
      </p>
      <div className="mt-5">
        <CodeBlock code={SERVER_TLS} title="tls-server.js" lang="js" />
      </div>
    </DocsShell>
  );
}
