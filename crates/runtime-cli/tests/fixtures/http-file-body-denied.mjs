// A file body without FileRead: the client gets a bare 500, the developer the
// refusal — never the file.
import { serve } from "runtime:http";
import { file } from "runtime:fs";

const server = serve({ port: 0, hostname: "127.0.0.1" }, () =>
  new Response(file(new URL("./static-body.txt", import.meta.url))),
);
const { port } = await server.addr;
const r = await fetch(`http://127.0.0.1:${port}/`);
console.log(`denied status:${r.status} body:${JSON.stringify(await r.text())}`);
await server.stop();
