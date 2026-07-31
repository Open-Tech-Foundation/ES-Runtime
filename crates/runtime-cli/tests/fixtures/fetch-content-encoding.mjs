// Content-Encoding end-to-end: a real runtime:http server returns genuinely
// compressed bytes and the real reqwest transport must hand the guest the
// decoded body, not the compressed one. Only the driven CLI can prove this —
// the stub transports in the runtime's own tests never compress anything.
import { serve } from "runtime:http";

const PLAIN = "hello ".repeat(64) + "world";

// Compress PLAIN with each coding, using the runtime's own CompressionStream.
async function compress(format) {
  const stream = new Blob([PLAIN]).stream().pipeThrough(new CompressionStream(format));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

const bodies = {
  gzip: await compress("gzip"),
  deflate: await compress("deflate"),
  br: await compress("brotli"),
};

const server = serve({ port: 0 }, (request) => {
  const { pathname, searchParams } = new URL(request.url);
  if (pathname === "/echo-accept") {
    return new Response(request.headers.get("accept-encoding") ?? "none");
  }
  if (pathname === "/echo-ua") {
    return new Response(request.headers.get("user-agent") ?? "none");
  }
  if (pathname === "/unknown-coding") {
    // Not actually zstd — the point is that the client leaves alone what it
    // cannot decode, rather than what it does with valid zstd.
    return new Response("opaque", { headers: { "content-encoding": "zstd" } });
  }
  const coding = searchParams.get("coding");
  return new Response(bodies[coding], {
    headers: { "content-encoding": coding, "content-type": "text/plain" },
  });
});

const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

// Every coding the client advertises must also be one it can decode.
for (const coding of ["gzip", "deflate", "br"]) {
  const r = await fetch(`${base}/body?coding=${coding}`);
  const text = await r.text();
  console.log(
    `DECODE ${coding} ok:${text === PLAIN}` +
      ` content-encoding:${r.headers.get("content-encoding")}` +
      ` content-length:${r.headers.get("content-length")}`,
  );
}

// The advertised set is what actually goes out on the wire.
const accept = await (await fetch(`${base}/echo-accept`)).text();
console.log(
  `ACCEPT gzip:${accept.includes("gzip")} br:${accept.includes("br")}` +
    ` deflate:${accept.includes("deflate")}`,
);

// Decoding keys off the response's Content-Encoding, not off who asked for it,
// so a caller that sets its own Accept-Encoding still gets a decoded body.
const explicit = await fetch(`${base}/body?coding=gzip`, {
  headers: { "accept-encoding": "gzip" },
});
console.log(`EXPLICIT ok:${(await explicit.text()) === PLAIN}`);

// A coding this client does not implement passes through untouched rather than
// being silently mangled — body and headers both stay as the server sent them.
const unknown = await fetch(`${base}/unknown-coding`);
console.log(
  `UNKNOWN body:${(await unknown.text()) === "opaque"}` +
    ` content-encoding:${unknown.headers.get("content-encoding")}`,
);

// The runtime identifies itself, and a caller can override that.
const ua = await (await fetch(`${base}/echo-ua`)).text();
console.log(`UA default:${ua.startsWith("ES-Runtime/")} matches-navigator:${ua === navigator.userAgent}`);
const custom = await (
  await fetch(`${base}/echo-ua`, { headers: { "user-agent": "custom/1" } })
).text();
console.log(`UA override:${custom === "custom/1"}`);

await server.stop();
console.log("ENCODING_OK");
