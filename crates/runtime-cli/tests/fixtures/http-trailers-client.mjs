// The round trip: a handler attaches trailers, and fetch reads them back. This
// is the gRPC shape end to end — status after the body, on one runtime.
import { serve, withTrailers, trailersOf } from "runtime:http";

const server = serve({ port: 0 }, (request) => {
  const { pathname } = new URL(request.url);
  if (pathname === "/ok") {
    return withTrailers(new Response("payload"), { "grpc-status": "0", "grpc-message": "fine" });
  }
  if (pathname === "/failed") {
    return withTrailers(new Response("partial"), Promise.resolve({ "grpc-status": "13" }));
  }
  return new Response("no trailers here");
});

const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

// The body must be read first: trailers are not on the wire before it ends.
const ok = await fetch(`${base}/ok`);
console.log(`body:${await ok.text()}`);
const okTrailers = await trailersOf(ok);
console.log(`status:${okTrailers.get("grpc-status")} message:${okTrailers.get("grpc-message")}`);

// A promised trailer set arrives the same way from the client's side.
const failed = await fetch(`${base}/failed`);
await failed.text();
console.log(`failed-status:${(await trailersOf(failed)).get("grpc-status")}`);

// A response with no trailers gives empty Headers, not a rejection.
const plain = await fetch(`${base}/plain`);
await plain.text();
const none = await trailersOf(plain);
console.log(`none:${[...none.keys()].length}`);

// A Response the guest built itself is not a fetch response — still no throw.
console.log(`local:${[...(await trailersOf(new Response("x"))).keys()].length}`);

// A body that is never read must not leave trailersOf pending forever, or a
// program that asks would simply hang.
const unread = await fetch(`${base}/ok`);
await unread.body.cancel();
console.log(`cancelled:${[...(await trailersOf(unread)).keys()].length}`);

let rejected = "none";
try {
  await trailersOf("not a response");
} catch (e) {
  rejected = e.constructor.name;
}
console.log(`bad-arg:${rejected}`);

await server.stop();
console.log("CLIENT_TRAILERS_OK");
