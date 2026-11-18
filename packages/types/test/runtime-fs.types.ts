// A type test for `runtime:fs`'s file handle. Compiled by `tsc -p .`, never run.

import { file } from "runtime:fs";

const handle = file("public/site.css");
// The media type its extension names, as `Blob.type` reports one.
const type: string = handle.type;
const path: string = handle.path;
// @ts-expect-error — `type` is read from the name, never assigned.
handle.type = "text/plain";
// The React fullstack template serves assets by it.
const served = new Response(handle.stream(), {
  headers: { "content-type": handle.type || "application/octet-stream" },
});

export { path, served, type };
