// What a failed bind tells the program.
//
// `serve()` returns a Server whose `addr` rejects asynchronously — but
// `finished` used to *resolve*, making a server that never bound
// indistinguishable from one that ran and shut down cleanly. The error also
// arrived as an uncoded "provider error: …" string, so there was nothing
// stable to branch on.
import { serve } from "runtime:http";

const held = serve({ port: 0, hostname: "127.0.0.1" }, () => new Response("held"));
const { port } = await held.addr;

const clash = serve({ port, hostname: "127.0.0.1" }, () => new Response("clash"));

try {
  await clash.addr;
  console.log("addr:NO-THROW");
} catch (e) {
  console.log(`addr:${e.code ?? "-"}:${e.message.startsWith("listen ")}`);
}

// The one that mattered: `finished` must not look like a clean shutdown.
try {
  await clash.finished;
  console.log("finished:RESOLVED");
} catch (e) {
  console.log(`finished:${e.code ?? "-"}`);
}

// The port's real owner is unaffected.
console.log(`held-still-serving:${await fetch(`http://127.0.0.1:${port}/`).then((r) => r.text())}`);
await held.stop();

// A clean shutdown still resolves `finished`, so rejecting is not the new
// default for every server.
const ok = serve({ port: 0, hostname: "127.0.0.1" }, () => new Response("ok"));
await ok.addr;
await ok.stop();
await ok.finished;
console.log("clean-finished:resolved");
console.log("BIND_FAILURE_OK");
