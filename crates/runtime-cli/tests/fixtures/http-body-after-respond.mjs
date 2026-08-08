// A request body kept past the response: the host drops an undrained body when
// the response goes out, so reading it afterwards should *end*, not error.
import { serve } from "runtime:http";

let leftover = null;
const server = serve({ port: 0, hostname: "127.0.0.1" }, (request) => {
  leftover = request.body;          // deliberately not drained
  return new Response("done");      // buffered — the host gives the request up here
});
const { port } = await server.addr;

// Big enough that one pre-pulled chunk cannot cover it: the stream must ask the
// host again, which is where this used to fail.
await fetch(`http://127.0.0.1:${port}/`, { method: "POST", body: "x".repeat(200_000) });

const reader = leftover.getReader();
let read = 0;
try {
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    read += value.length;
  }
  console.log(`ENDED_CLEANLY read=${read > 0}`);
} catch (e) {
  console.log("THREW", e.code ?? e.name);
}
await server.stop();
