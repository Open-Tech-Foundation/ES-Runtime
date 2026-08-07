// A top-level throw with a listener already open. The failure was only checked
// after the drive loop returned, and a server keeps that loop alive forever, so
// the exception was discarded entirely and the process ran on serving requests
// — no message, no exit. It must be fatal, exactly as it is without a server.
import { serve } from "runtime:http";

const server = serve({ port: 0, hostname: "127.0.0.1" }, () => new Response("x"));
await server.addr;
throw new Error("top-level failure while serving");
