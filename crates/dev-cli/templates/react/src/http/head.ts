/**
 * The `<title>` and `<meta>` a route asked for, as text.
 *
 * It imports nothing, which is what makes it testable: `esdev test` runs each
 * file unbundled, and anything that reaches React reaches CommonJS, which the
 * runtime does not load. Everything here is a string in and a string out, and
 * the part that matters is escaping — so it lives where a test can reach it
 * rather than beside the file read in `src/document.ts`.
 */

/** What a route's `handle.meta` produces. */
export type Meta = {
  title: string;
  description?: string;
};

/** One matched route, as much of it as choosing a title depends on. */
export type MetaSource = {
  meta?: (data: unknown) => Meta;
  data: unknown;
};

/**
 * The `Meta` for a set of matched routes: the deepest one that describes any.
 *
 * Deepest-first, so a specific route wins over the layout it sits in — the same
 * way every other nested thing in a router resolves.
 *
 * Shared by the server (which writes the tags into the document) and the
 * browser (which sets `document.title` on a client-side navigation). One
 * function, because two would be two answers to "what is this page called", and
 * they would drift the first time a route was added.
 *
 * `data` is whatever the route's loader returned, and is `undefined` for a
 * route that has none — so a `meta` that reads it belongs on a route that has
 * one. The *error* case never reaches here: a route that threw renders its
 * ErrorBoundary instead, and that page names itself.
 */
export function pickMeta(matches: readonly MetaSource[], fallback: Meta): Meta {
  for (let i = matches.length - 1; i >= 0; i--) {
    const match = matches[i];
    if (match?.meta) {
      return match.meta(match.data);
    }
  }
  return fallback;
}

/** `meta`, as the tags that go inside `<head>`. */
export function head(meta: Meta): string {
  const tags = [`<title>${escape(meta.title)}</title>`];
  if (meta.description) {
    tags.push(`<meta name="description" content="${escape(meta.description)}">`);
  }
  return tags.join("");
}

/**
 * Text going into an HTML document, as text.
 *
 * A page title is very often somebody else's string — a post's title, a search
 * term echoed back, a name from a database. `<` and `&` are the two that change
 * how the document parses; `"` matters because this lands in an attribute value
 * as well as in an element.
 */
export function escape(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}
