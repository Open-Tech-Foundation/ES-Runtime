// Fetch's network errors must reject with a TypeError — that is how a caller is
// meant to tell a transport failure from a programming mistake. The host ops
// raise a plain Error, so connection-refused, an unsupported scheme and a
// redirect loop all failed `e instanceof TypeError` while `redirect: "error"`
// passed it: one operation reporting two different ways.
//
// Aborts and capability denials are deliberately not network errors and keep
// their own types.
import { serve } from "runtime:http";

const server = serve({ port: 0, hostname: "127.0.0.1" }, (req) => {
  const u = new URL(req.url);
  if (u.pathname === "/loop") return Response.redirect(`http://127.0.0.1:${port}/loop`, 302);
  if (u.pathname === "/slow") return new Promise(() => {}); // never settles
  return new Response("ok");
});
const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

async function report(label, fn) {
  try {
    await fn();
    console.log(`${label}:NO-THROW`);
  } catch (e) {
    console.log(`${label}:${e.constructor.name}:${e.name}:${e.code ?? "-"}`);
  }
}

await report("refused", () => fetch("http://127.0.0.1:1/"));
await report("badscheme", () => fetch("ftp://example.invalid/"));
await report("dns", () => fetch("http://no.such.host.invalid/"));
await report("loop", () => fetch(`${base}/loop`));
await report("redirect-error-mode", () => fetch(`${base}/loop`, { redirect: "error" }));
await report("relative", () => fetch("/relative"));

// Not network errors: these keep their own identity.
await report("aborted", () => fetch(`${base}/slow`, { signal: AbortSignal.abort() }));
await report("timeout", () => fetch(`${base}/slow`, { signal: AbortSignal.timeout(50) }));

// …and a working request is unaffected.
console.log(`ok:${(await fetch(base)).status}`);

await server.stop();
console.log("NETWORK_ERRORS_OK");
