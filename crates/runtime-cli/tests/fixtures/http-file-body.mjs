// `new Response(file(path))` (D145): `serve` sends the file host-side, and every
// other reader of the same body — `text()`, `.body`, `clone()`, a fetch upload —
// still sees the file's bytes.
import { serve } from "runtime:http";
import { file } from "runtime:fs";

const body = () => file(new URL("./static-body.txt", import.meta.url));

const server = serve({ port: 0, hostname: "127.0.0.1" }, async (req) => {
  switch (new URL(req.url).pathname) {
    case "/file":
      return new Response(body(), { status: 201, headers: { "x-kind": "file" } });
    case "/missing":
      return new Response(file(new URL("./no-such-file.txt", import.meta.url)));
    case "/clone": {
      const original = new Response(body());
      const copy = original.clone();
      console.log(`clone original:${JSON.stringify(await original.text())}`);
      return copy;
    }
    case "/echo":
      return new Response(await req.text());
  }
});
const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

const r = await fetch(`${base}/file`);
console.log(`file status:${r.status} kind:${r.headers.get("x-kind")} body:${JSON.stringify(await r.text())}`);

const missing = await fetch(`${base}/missing`);
console.log(`missing status:${missing.status} body:${JSON.stringify(await missing.text())}`);

const cloned = await fetch(`${base}/clone`);
console.log(`clone status:${cloned.status} body:${JSON.stringify(await cloned.text())}`);

const echoed = await fetch(`${base}/echo`, { method: "POST", body: body() });
console.log(`upload body:${JSON.stringify(await echoed.text())}`);

// Outside `serve` a file body is read like any other.
console.log(`text:${JSON.stringify(await new Response(body()).text())}`);
const chunks = [];
for await (const chunk of new Response(body()).body) chunks.push(...chunk);
console.log(`stream:${JSON.stringify(new TextDecoder().decode(new Uint8Array(chunks)))}`);

await server.stop();
console.log("FILE_BODY_OK");
