/**
 * The built `index.html`, in the pieces a response is assembled from.
 *
 * It is read from **beside the bundle**: a relative path resolves against the
 * entry module's directory, so `dist/server.js` reads `dist/index.html` — the
 * built one, whose script and stylesheet URLs are already the hashed names.
 *
 * `<!--head-->…<!--/head-->` wraps a default head the server replaces per
 * route; `<!--app-->` is where the render goes.
 */
import { file } from "runtime:fs";
import { dirname, fromFileURL, join } from "runtime:path";

/** The directory the running bundle is in. */
export const here = dirname(fromFileURL(import.meta.url));

const template = await file(join(here, "index.html")).text();

function split(text: string, marker: string): [string, string] {
  const at = text.indexOf(marker);
  if (at < 0) {
    // A build that lost a marker produces a server that answers every request
    // with an empty page, and nothing else would notice. Better not to start.
    throw new Error(
      `index.html has no ${marker}. The server splices its render into the built ` +
        `document, and that marker is where it goes.`,
    );
  }
  return [text.slice(0, at), text.slice(at + marker.length)];
}

const [beforeHead, fromHead] = split(template, "<!--head-->");
const [, afterHead] = split(fromHead, "<!--/head-->");
const [beforeApp, afterApp] = split(afterHead, "<!--app-->");

/**
 * The document in the three spans a response is built from: everything before
 * the head, everything between the head and the app, and everything after.
 */
export const document = { beforeHead, beforeApp, afterApp };
