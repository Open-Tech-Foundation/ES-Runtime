// serve()'s `onError` and the unhandled report (D206).
import { serve } from "runtime:http";
import { currentTask } from "runtime:context";

const json = serve({
  hostname: "127.0.0.1",
  port: 0,
  onError: (error, request) =>
    Response.json(
      { error: error.message, path: new URL(request.url).pathname, trace: currentTask().traceId.length },
      { status: 503 },
    ),
}, (request) => {
  if (new URL(request.url).pathname === "/object") return { ok: true };
  throw new Error("handled by onError");
});
const broken = serve({
  hostname: "127.0.0.1",
  port: 0,
  onError: () => { throw new Error("onError itself failed"); },
}, () => { throw new Error("first failure"); });
const plain = serve({ hostname: "127.0.0.1", port: 0 }, () => {
  throw new RangeError("unhandled in plain");
});

async function hit(server, path) {
  const { port } = await server.addr;
  const response = await fetch(`http://127.0.0.1:${port}${path}`);
  return `${path} ${response.status} ${await response.text()}`;
}

console.log(await hit(json, "/orders/1?token=secret"));
console.log(await hit(json, "/object"));
console.log(await hit(broken, "/b"));
console.log(await hit(plain, "/p?token=secret"));
try {
  serve({ onError: "nope" }, () => new Response());
} catch (e) {
  console.log(`option:${e.name}:${e.message}`);
}
for (const server of [json, broken, plain]) await server.stop();
console.log("ON_ERROR_OK");
