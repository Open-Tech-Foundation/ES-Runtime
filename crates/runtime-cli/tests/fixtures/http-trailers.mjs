// Trailers from a real handler, read off the wire. This is the gRPC shape: the
// status of a call is not known until the body has been produced, so it travels
// after it.
import { serve, withTrailers } from "runtime:http";
import { connect } from "runtime:net";

const server = serve({ port: 0 }, (request) => {
  const { pathname } = new URL(request.url);

  if (pathname === "/buffered") {
    // Trailers in hand: the body is complete, so the names can be declared for
    // HTTP/1.1 automatically.
    return withTrailers(new Response("hello"), { "grpc-status": "0", "grpc-message": "ok" });
  }

  if (pathname === "/streamed") {
    // The realistic shape: a promise that settles only after the body has been
    // produced, because its value depends on how that went.
    let finish;
    const status = new Promise((resolve) => (finish = resolve));
    const body = new ReadableStream({
      async start(c) {
        c.enqueue(new TextEncoder().encode("chunk1;"));
        c.enqueue(new TextEncoder().encode("chunk2;"));
        c.close();
        finish({ "grpc-status": "0" });
      },
    });
    // HTTP/1.1 only carries trailer fields the head declared, and the head is
    // long gone by the time the promise settles — so declare them here.
    return withTrailers(new Response(body, { headers: { trailer: "grpc-status" } }), status);
  }

  return new Response("plain");
});

const { port } = await server.addr;

// A raw socket, because fetch drops trailers — as every runtime's does.
async function wire(path) {
  const socket = await connect({ hostname: "127.0.0.1", port });
  const writer = socket.writable.getWriter();
  await writer.write(
    new TextEncoder().encode(
      `GET ${path} HTTP/1.1\r\nHost: x\r\nTE: trailers\r\nConnection: close\r\n\r\n`,
    ),
  );
  writer.releaseLock();
  const reader = socket.readable.getReader();
  let out = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    out += new TextDecoder().decode(value);
  }
  return out;
}

const buffered = await wire("/buffered");
console.log(`buffered-trailer:${buffered.includes("grpc-status: 0")}`);
// Declared automatically, since the names were known before the head went out.
console.log(`buffered-declared:${/trailer: grpc-status/i.test(buffered)}`);
console.log(`buffered-chunked:${/transfer-encoding: chunked/i.test(buffered)}`);

const streamed = await wire("/streamed");
console.log(`streamed-body:${streamed.includes("chunk1;") && streamed.includes("chunk2;")}`);
console.log(`streamed-trailer:${streamed.includes("grpc-status: 0")}`);

// A response with no trailers is untouched by any of this.
const plain = await wire("/plain");
console.log(`plain-ok:${plain.includes("plain") && !plain.includes("grpc-status")}`);

// Bad input is rejected at the call.
let rejected = "none";
try {
  withTrailers("not a response", { a: "b" });
} catch (e) {
  rejected = e.constructor.name;
}
console.log(`bad-arg:${rejected}`);

await server.stop();
console.log("TRAILERS_OK");
