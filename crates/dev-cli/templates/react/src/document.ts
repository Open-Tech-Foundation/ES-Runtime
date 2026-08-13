/**
 * The built `index.html`, split at the two markers the server fills in.
 *
 * It is read from beside the bundle rather than from the source tree: the
 * runtime resolves a relative path against the *entry module's* directory, so
 * `dist/server.js` reading `index.html` reads `dist/index.html` — the built one,
 * with its script and stylesheet URLs already rewritten. The source document is
 * not deployed and, on the machine that runs this, is not there at all.
 */
import { file } from "runtime:fs";
import { dirname, fromFileURL, join } from "runtime:path";

export { serialize } from "./serialize.ts";

/** The directory the running bundle is in. */
export const here = dirname(fromFileURL(import.meta.url));

const template = await file(join(here, "index.html")).text();
const [beforeApp, rest] = template.split("<!--app-->");
const [afterApp, afterData] = rest.split("<!--data-->");

/** The document, in the three pieces a render is spliced into. */
export const document = { beforeApp, afterApp, afterData };
