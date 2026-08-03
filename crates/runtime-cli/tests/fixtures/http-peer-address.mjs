// The handler's second argument over a real connection: the peer it reports is
// the actual other end of the socket, not anything the client claimed in a
// header. Only the driven CLI can show this — it needs a real accept().
import { serve } from "runtime:http";

const server = serve({ port: 0 }, (request, info) => {
  const { pathname } = new URL(request.url);

  if (pathname === "/who") {
    const peer = info.remoteAddr;
    return Response.json({
      transport: peer.transport,
      hostname: peer.hostname,
      // The port is ephemeral, so only its shape can be asserted.
      hasPort: Number.isInteger(peer.port) && peer.port > 0,
    });
  }

  if (pathname === "/forwarded") {
    // The header is delivered untouched, so a deployment that knows which hop
    // to trust can resolve it itself — the runtime just never does it for you.
    return new Response(request.headers.get("x-forwarded-for") ?? "absent");
  }

  // A handler that never looks at `info` is unaffected by its existence.
  return new Response("ignored");
});

const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

const who = await (await fetch(`${base}/who`)).json();
console.log(`peer:${who.transport}/${who.hostname} hasPort:${who.hasPort}`);

// A forged forwarding header must change nothing: the peer comes from the
// socket, and a header anyone can send is not an identity.
const forged = await (
  await fetch(`${base}/who`, { headers: { "x-forwarded-for": "198.51.100.9" } })
).json();
console.log(`forged-ignored:${forged.hostname === who.hostname}`);

// …while the header itself is still delivered, so a deployment that knows which
// hop to trust can do its own resolution.
const forwarded = await fetch(`${base}/forwarded`, {
  headers: { "x-forwarded-for": "198.51.100.9" },
});
console.log(`header-delivered:${await forwarded.text()}`);

// A one-argument handler is unaffected by the second argument existing.
console.log(`one-arg:${await (await fetch(`${base}/other`)).text()}`);

await server.stop();
console.log("PEER_OK");
