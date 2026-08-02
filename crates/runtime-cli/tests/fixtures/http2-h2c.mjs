// A cleartext `serve()` answers an HTTP/2 client that opens with the connection
// preface (h2c by prior knowledge). The client here is hand-rolled over
// `runtime:net` on purpose: `fetch` speaks HTTP/1.1 to a cleartext origin, so
// only raw frames can show that the listener really switched protocols rather
// than answering HTTP/1.1 as it always did.
//
// Nothing is decompressed — HPACK is only *written* here (static-table indexes
// and plain literals, no dynamic table), and what is read back is the framing
// itself plus DATA payloads, which travel as-is. That is enough to prove four
// things end-to-end through the real binary: the version detection fires, the
// server's SETTINGS carry the stream cap we advertise, two streams are served
// concurrently on one connection, and a request body sent as DATA frames
// reaches the handler.
import { serve } from "runtime:http";
import { connect } from "runtime:net";

const server = serve({ hostname: "127.0.0.1", port: 0 }, async (request) => {
  const path = new URL(request.url).pathname;
  if (request.method === "POST") {
    const body = await request.text();
    return new Response(`echo:${body}`);
  }
  return new Response(`body-for:${path}`);
});
const { port } = await server.addr;

const enc = new TextEncoder();
const dec = new TextDecoder();

// One HTTP/2 frame: 24-bit length, type, flags, 31-bit stream id, payload.
function frame(type, flags, streamId, payload) {
  const out = new Uint8Array(9 + payload.length);
  out[0] = (payload.length >> 16) & 0xff;
  out[1] = (payload.length >> 8) & 0xff;
  out[2] = payload.length & 0xff;
  out[3] = type;
  out[4] = flags;
  out[5] = (streamId >>> 24) & 0x7f;
  out[6] = (streamId >>> 16) & 0xff;
  out[7] = (streamId >>> 8) & 0xff;
  out[8] = streamId & 0xff;
  out.set(payload, 9);
  return out;
}

// HPACK "literal header field never indexed" with the name taken from the
// static table (`0x10 | index`), then an unhuffmanned length-prefixed value.
function literal(nameIndex, value) {
  const v = enc.encode(value);
  return [0x10 | nameIndex, v.length, ...v];
}

// The pseudo-header block every request here starts with. `method` picks the
// static-table index for GET (2) or POST (3); both are indexed, so no literal.
function headerBlock(method, path) {
  return new Uint8Array([
    method === "POST" ? 0x83 : 0x82, // :method
    0x86, // :scheme http (static index 6)
    ...literal(4, path), // :path
    ...literal(1, `127.0.0.1:${port}`), // :authority — h2 has no Host header
  ]);
}

const socket = connect({ hostname: "127.0.0.1", port });
await socket.opened;
const writer = socket.writable.getWriter();
const reader = socket.readable.getReader();

await writer.write(enc.encode("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"));
await writer.write(frame(0x4, 0x0, 0, new Uint8Array(0))); // empty SETTINGS

// Three streams opened back-to-back, before a single response is read. On
// HTTP/1.1 that is not expressible on one connection; here the server may
// answer them in any order, which is exactly what the assertions allow.
const END_STREAM = 0x1;
const END_HEADERS = 0x4;
await writer.write(frame(0x1, END_HEADERS | END_STREAM, 1, headerBlock("GET", "/h2c")));
await writer.write(frame(0x1, END_HEADERS | END_STREAM, 3, headerBlock("GET", "/second")));
// Stream 5 carries a body: HEADERS without END_STREAM, then a DATA frame.
await writer.write(frame(0x1, END_HEADERS, 5, headerBlock("POST", "/upload")));
await writer.write(frame(0x0, END_STREAM, 5, enc.encode("uploaded-bytes")));

let buf = new Uint8Array(0);
const headersSeen = new Set();
const bodies = new Map([
  [1, ""],
  [3, ""],
  [5, ""],
]);
const finished = new Set();
let maxConcurrentStreams = null;

while (finished.size < 3) {
  const { value, done: eof } = await reader.read();
  if (eof) break;
  const grown = new Uint8Array(buf.length + value.length);
  grown.set(buf);
  grown.set(value, buf.length);
  buf = grown;

  // Drain every whole frame the buffer now holds; a partial tail waits.
  while (buf.length >= 9) {
    const len = (buf[0] << 16) | (buf[1] << 8) | buf[2];
    if (buf.length < 9 + len) break;
    const type = buf[3];
    const flags = buf[4];
    const stream = ((buf[5] & 0x7f) << 24) | (buf[6] << 16) | (buf[7] << 8) | buf[8];
    const payload = buf.subarray(9, 9 + len);
    if (type === 0x4 && (flags & 0x1) === 0) {
      // SETTINGS: 6 bytes per entry — a 16-bit id and a 32-bit value. Id 0x3 is
      // MAX_CONCURRENT_STREAMS, the cap the server puts on one connection.
      for (let i = 0; i + 6 <= payload.length; i += 6) {
        const id = (payload[i] << 8) | payload[i + 1];
        const val =
          payload[i + 2] * 0x1000000 +
          ((payload[i + 3] << 16) | (payload[i + 4] << 8) | payload[i + 5]);
        if (id === 0x3) maxConcurrentStreams = val;
      }
      await writer.write(frame(0x4, 0x1, 0, new Uint8Array(0))); // SETTINGS ACK
    } else if (type === 0x1 && bodies.has(stream)) {
      headersSeen.add(stream);
      if (flags & END_STREAM) finished.add(stream);
    } else if (type === 0x0 && bodies.has(stream)) {
      bodies.set(stream, bodies.get(stream) + dec.decode(payload));
      if (flags & END_STREAM) finished.add(stream);
    }
    buf = buf.slice(9 + len);
  }
}

console.log(headersSeen.size === 3 ? "H2C_HEADERS_FRAME" : "H2C_NO_HEADERS_FRAME");
console.log(`h2c-body:${bodies.get(1)}`);
console.log(`h2c-second:${bodies.get(3)}`);
console.log(`h2c-post:${bodies.get(5)}`);
console.log(`h2c-max-streams:${maxConcurrentStreams}`);

// The same port still answers HTTP/1.1 — `fetch` is a plain HTTP/1.1 client
// here, so this is the mixed-version case a running deployment lands in the
// moment h2 is switched on.
const one = await fetch(`http://127.0.0.1:${port}/http1`);
console.log(`h1-body:${await one.text()}`);

await socket.close();
await server.stop();
