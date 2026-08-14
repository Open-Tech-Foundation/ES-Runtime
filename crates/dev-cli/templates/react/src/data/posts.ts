/**
 * Stands in for whatever this app actually reads from.
 *
 * It is a module with `async` functions rather than an array the routes index
 * directly, because that is the shape a real source has — `runtime:db`, a fetch
 * to a service, a file read. Swapping this file for one of those should not
 * change a single line of a route.
 */

export type Post = {
  slug: string;
  title: string;
  summary: string;
  body: string[];
  published: string;
};

const POSTS: Post[] = [
  {
    slug: "server-rendering",
    title: "One render, three ways to ship it",
    summary: "The same components answer a request, hydrate a page, and write a file.",
    published: "2026-01-12",
    body: [
      "One file — src/render.tsx — turns a matched route into HTML. The server calls it per request, the prerender step calls it once per route at build time, and the browser hydrates whatever came out.",
      "That is deliberate. Two render paths means two ways for a page to be subtly different depending on how it was produced, and the difference always shows up somewhere expensive.",
    ],
  },
  {
    slug: "permissions",
    title: "The server runs on what it was granted",
    summary: "No filesystem beyond dist, no network, no subprocesses — in development too.",
    published: "2026-01-19",
    body: [
      "esdev.json names the permissions this project needs, and esdev start runs the server under exactly them. There is no permissive development mode that quietly widens to --allow-all, because a grant you never exercise in development is a grant you discover in production.",
      "Run esdev --trace-permissions dist/server.js to see what a run actually used. It prints the flag line, and it is usually shorter than what you asked for.",
    ],
  },
  {
    slug: "loaders",
    title: "Data belongs to the route",
    summary: "A loader runs before the component, on the server and on the client.",
    published: "2026-01-26",
    body: [
      "This page's text arrived from a loader on src/routes.tsx. On a full page load the server ran it, rendered with the result, and sent the data along in the document so the browser did not have to ask twice. On a client-side navigation the browser ran the same loader itself.",
      "Neither of those is something a component has to know about, which is the point.",
    ],
  },
];

/** Every post, newest first. */
export async function listPosts(): Promise<Post[]> {
  return [...POSTS].sort((a, b) => b.published.localeCompare(a.published));
}

/** One post, or `undefined` if there is no such slug. */
export async function findPost(slug: string): Promise<Post | undefined> {
  return POSTS.find((post) => post.slug === slug);
}
