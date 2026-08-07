// SO_REUSEPORT through both doors: several listeners share one port and the
// kernel balances across them. Without the option the second bind is refused,
// which is what makes this a test of the option rather than of a free port.
import { serve } from "runtime:http";
import { listen } from "runtime:net";

// ---- runtime:http -----------------------------------------------------------
const a = serve({ port: 0, hostname: "127.0.0.1", reusePort: true }, () => new Response("a"));
const { port } = await a.addr;
const b = serve({ port, hostname: "127.0.0.1", reusePort: true }, () => new Response("b"));
const second = await b.addr;
console.log(`http-shared:${second.port === port}`);

// Whichever accepts, a request on that port is served.
const body = await fetch(`http://127.0.0.1:${port}/`).then((r) => r.text());
console.log(`http-answered:${body === "a" || body === "b"}`);

// …and without the option, the same bind is refused.
try {
  await serve({ port, hostname: "127.0.0.1" }, () => new Response("c")).addr;
  console.log("http-exclusive:NO-THROW");
} catch (e) {
  console.log(`http-exclusive:${e.code ?? e.name}`);
}
await a.stop();
await b.stop();

// ---- runtime:net ------------------------------------------------------------
const l1 = listen({ port: 0, hostname: "127.0.0.1", reusePort: true });
const { port: p2 } = await l1.addr;
const l2 = listen({ port: p2, hostname: "127.0.0.1", reusePort: true });
console.log(`net-shared:${(await l2.addr).port === p2}`);
try {
  await listen({ port: p2, hostname: "127.0.0.1" }).addr;
  console.log("net-exclusive:NO-THROW");
} catch (e) {
  console.log(`net-exclusive:${e.code ?? e.name}`);
}
await l1.close();
await l2.close();

// A non-boolean is a mistake at the call, not a silently ignored option.
for (const [label, mod] of [["http", serve], ["net", listen]]) {
  try {
    mod === serve
      ? serve({ port: 0, hostname: "127.0.0.1", reusePort: "yes" }, () => new Response(""))
      : listen({ port: 0, hostname: "127.0.0.1", reusePort: "yes" });
    console.log(`${label}-bad-option:NO-THROW`);
  } catch (e) {
    console.log(`${label}-bad-option:${e.name}`);
  }
}
console.log("REUSE_PORT_OK");
