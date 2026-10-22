// `file().type` is the media type its extension names, as `Blob.type` is — ""
// when unknown — and a file body takes its `Content-Type` from it, the way a
// Blob body does. A header the handler sets wins.
import { serve } from "runtime:http";
import { file } from "runtime:fs";

const css = () => file(new URL("./static-site.css", import.meta.url));

console.log(`types:${JSON.stringify(["a.css", "x/y.HTML", "data.bin", ".env", "Makefile"].map((p) => file(p).type))}`);

const server = serve({ port: 0, hostname: "127.0.0.1" }, async (req) => {
  switch (new URL(req.url).pathname) {
    case "/css":
      return new Response(css());
    case "/own":
      return new Response(css(), { headers: { "content-type": "text/plain" } });
    case "/echo":
      return new Response(req.headers.get("content-type"));
  }
});
const { port } = await server.addr;
const base = `http://127.0.0.1:${port}`;

const served = await fetch(`${base}/css`);
console.log(`served:${served.headers.get("content-type")} body:${JSON.stringify(await served.text())}`);
const own = await fetch(`${base}/own`);
console.log(`own:${own.headers.get("content-type")}`);
const upload = await fetch(`${base}/echo`, { method: "POST", body: css() });
console.log(`upload:${await upload.text()}`);
console.log(`local:${new Response(css()).headers.get("content-type")}`);

await server.stop();
console.log("FILE_TYPE_OK");
