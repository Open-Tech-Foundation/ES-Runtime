/**
 * Serving what the build wrote to `dist/assets`.
 *
 * The decisions here are the ones that are silent when they are wrong: a
 * missing content type makes a stylesheet arrive as text and the page render
 * unstyled, and a path that is allowed to climb turns a static file server into
 * a way to read the filesystem.
 *
 * The two that need testing take no I/O and are exported for it.
 */

/** Where the build puts everything it hashed. */
export const ASSET_PREFIX = "/assets/";

/**
 * Content types for what the build writes.
 *
 * A short list on purpose: these are the extensions esdev emits, and anything
 * else falls back to a type that makes a browser download rather than guess.
 * `charset` is on the text types because without it the browser falls back to
 * its locale's encoding, and the first non-ASCII character in the CSS shows it.
 */
const TYPES: Record<string, string> = {
  js: "text/javascript; charset=utf-8",
  mjs: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  map: "application/json; charset=utf-8",
  json: "application/json; charset=utf-8",
  html: "text/html; charset=utf-8",
  txt: "text/plain; charset=utf-8",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  avif: "image/avif",
  gif: "image/gif",
  ico: "image/x-icon",
  woff: "font/woff",
  woff2: "font/woff2",
  ttf: "font/ttf",
  otf: "font/otf",
  wasm: "application/wasm",
};

/** The content type for a filename, by extension. */
export function contentType(name: string): string {
  const dot = name.lastIndexOf(".");
  const extension = dot < 0 ? "" : name.slice(dot + 1).toLowerCase();
  return TYPES[extension] ?? "application/octet-stream";
}

/**
 * Whether `name` is a filename this server will look up under `assets/`.
 *
 * It **rejects** rather than sanitises. Stripping `..` out of a path is a game
 * of finding every spelling of it — encoded, doubled, backslashed, embedded in
 * a NUL — and the only move that wins is not playing: an asset written by the
 * build is one path segment, so anything with structure in it is not one.
 *
 * The request has already been percent-decoded by `URL`, so `%2e%2e%2f` arrives
 * here as `../` and is caught by the same rule.
 */
export function isAssetName(name: string): boolean {
  return (
    name.length > 0 &&
    name.length < 256 &&
    !name.includes("/") &&
    !name.includes("\\") &&
    !name.includes("\0") &&
    name !== "." &&
    name !== ".."
  );
}

/**
 * How long a browser may keep an asset.
 *
 * Everything a real build writes carries a content hash in its name, so it can
 * be kept for ever: a changed file gets a changed name, and the old URL is
 * never asked for again. `esdev start` does not hash — it rebuilds the same
 * filenames in place — so the same answer there would mean editing a stylesheet
 * and seeing no change for a year.
 *
 * The two go together exactly, because esdev decides both from the same thing:
 * a development build skips the hash *and* defines `NODE_ENV` as
 * `"development"`. So this reads the flag rather than trying to recognise a
 * hash in a filename — guessing wrong in the cacheable direction is a mistake
 * with no way back, since the browsers that took the answer are not reachable
 * to correct it.
 */
export function cacheControl(): string {
  return process.env.NODE_ENV === "production"
    ? "public, max-age=31536000, immutable"
    : "no-cache";
}
