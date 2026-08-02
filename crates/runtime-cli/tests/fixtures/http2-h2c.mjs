// A cleartext `serve()` answers an HTTP/2 client that opens with the connection
// preface (h2c by prior knowledge). The client here is hand-rolled over
// `runtime:net` on purpose: `fetch` speaks HTTP/1.1 to a cleartext origin, so
// only raw frames can show that the listener really switched protocols rather
// than answering HTTP/1.1 as it always did. Nothing is decompressed — HPACK is
// only *written* here (static-table indexes and plain literals, no dynamic
// table), and the proof read back is the framing itself plus the DATA payload,
// which travels as-is.
import { serve } from "runtime:http";
import { connect } from "runtime:net";

const server = serve(
  { hostname: "127.0.0.1", port: 0 },
  (request) => new Response(`body-for:${new URL(request.url).pathname}`),
);
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

const socket = connect({ hostname: "127.0.0.1", port });
await socket.opened;
const writer = socket.writable.getWriter();
const reader = socket.readable.getReader();

await writer.write(enc.encode("PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"));
await writer.write(frame(0x4, 0x0, 0, new Uint8Array(0))); // empty SETTINGS
await writer.write(
  frame(
    0x1, // HEADERS
    0x4 | 0x1, // END_HEADERS | END_STREAM (no request body)
    1,
    new Uint8Array([
      0x82, // :method GET   (static index 2, indexed)
      0x86, // :scheme http  (static index 6, indexed)
      ...literal(4, "/h2c"), // :path
      ...literal(1, `127.0.0.1:${port}`), // :authority — h2 has no Host header
    ]),
  ),
);

let buf = new Uint8Array(0);
let sawHeaders = false;
let body = "";
let done = false;

while (!done) {
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
      await writer.write(frame(0x4, 0x1, 0, new Uint8Array(0))); // SETTINGS ACK
    } else if (type === 0x1 && stream === 1) {
      sawHeaders = true;
    } else if (type === 0x0 && stream === 1) {
      body += dec.decode(payload);
      if (flags & 0x1) done = true; // END_STREAM
    }
    buf = buf.slice(9 + len);
  }
}

console.log(sawHeaders ? "H2C_HEADERS_FRAME" : "H2C_NO_HEADERS_FRAME");
console.log(`h2c-body:${body}`);

await socket.close();
await server.stop();
