// An unhandled rejection in a program that never quiesces.
//
// Failures used to be collected and printed when the drive returned — which for
// a listening server is never, so a long-running program's failures were
// invisible for its whole life and arrived only at exit. This prints markers
// around the rejection so the *order* can be checked: the report has to land
// between them, while the server is still up.
import { serve } from "runtime:http";

const server = serve({ port: 0, hostname: "127.0.0.1" }, () => new Response("x"));
await server.addr;
console.log("MARK_BEFORE");

setTimeout(() => {
  Promise.reject(new TypeError("failed while serving"));
}, 50);

setTimeout(async () => {
  console.log("MARK_AFTER");
  await server.stop();
}, 400);
