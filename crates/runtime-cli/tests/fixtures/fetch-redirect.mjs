// Redirect modes end-to-end: a real runtime:http server issues real 3xx
// responses and the real reqwest transport is asked to follow them, hand them
// back, or refuse them. The in-process runtime tests use a stub transport, so
// this is the one that proves the wire behaviour.
import { serve } from "runtime:http";

let base;
const server = serve({ port: 0 }, (request) => {
  const { pathname } = new URL(request.url);
  // /hop/N redirects to /hop/(N+1); /landed is the end of the chain.
  if (pathname.startsWith("/hop/")) {
    const n = Number(pathname.slice("/hop/".length));
    const next = n >= 2 ? `${base}/landed` : `${base}/hop/${n + 1}`;
    return new Response(null, { status: 302, headers: { location: next } });
  }
  if (pathname === "/loop") {
    return new Response(null, { status: 302, headers: { location: `${base}/loop` } });
  }
  return new Response("landed");
});

const { port } = await server.addr;
base = `http://127.0.0.1:${port}`;

// follow (the default): the whole chain is walked, and the response reports
// where it actually ended up.
const followed = await fetch(`${base}/hop/0`);
console.log(
  `FOLLOW status:${followed.status} redirected:${followed.redirected}` +
    ` landed:${followed.url.endsWith("/landed")} body:${await followed.text()}`,
);

// manual: the 3xx itself, with its Location intact and nothing followed.
const manual = await fetch(`${base}/hop/0`, { redirect: "manual" });
console.log(
  `MANUAL status:${manual.status} redirected:${manual.redirected}` +
    ` location:${manual.headers.get("location")?.endsWith("/hop/1")}`,
);

// error: a redirect the caller asked never to see is a network error.
let errored = "none";
try {
  await fetch(`${base}/hop/0`, { redirect: "error" });
} catch (e) {
  errored = e.constructor.name;
}
console.log(`ERROR threw:${errored}`);

// A response that never redirected says so.
const direct = await fetch(`${base}/landed`);
console.log(`DIRECT redirected:${direct.redirected}`);

// An endless chain stops at the spec's cap rather than spinning forever.
let loopCode = "none";
try {
  await fetch(`${base}/loop`);
} catch (e) {
  loopCode = e.code ?? e.constructor.name;
}
console.log(`LOOP code:${loopCode}`);

await server.stop();
console.log("REDIRECT_OK");
