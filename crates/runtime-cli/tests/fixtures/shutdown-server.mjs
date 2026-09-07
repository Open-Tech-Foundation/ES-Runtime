// A server with a deliberately slow handler, for the graceful-shutdown tests.
// The parent prints-and-waits on PORT, fires a request at /slow, then sends a
// real interrupt while that request is still being handled.
import { serve } from "runtime:http";

const server = serve({ port: 0 }, async (request) => {
  if (new URL(request.url).pathname === "/slow") {
    // Observed by the parent, which interrupts only once this is in flight: a
    // fixed sleep there would race a loaded machine's dispatch of the request.
    console.log("SLOW ENTERED");
    await new Promise((resolve) => setTimeout(resolve, 1200));
    return new Response("slow finished");
  }
  return new Response("ok");
});

const { port } = await server.addr;
console.log(`PORT ${port}`);

await server.finished;
console.log("SERVER FINISHED");
