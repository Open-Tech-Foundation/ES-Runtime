/**
 * Every path the static build writes as a file.
 *
 * A static build has to know its own URLs, and a route table with a `:slug` in
 * it does not: `posts/:slug` is a pattern, and only the data behind it says
 * which pages exist. So the expansion happens here, against the same data the
 * loaders read, rather than in the build script where it would drift.
 *
 * **A path left out of this list is not a broken page.** It is a route the
 * browser renders instead — served the shell, matched client-side, its loader
 * run there. So this is the list of pages worth having as HTML on the host,
 * which is not always every page an app has: a dashboard behind a login has
 * nothing to prerender.
 */
import { listPosts } from "./data/posts.ts";

export async function staticPaths(): Promise<string[]> {
  const posts = await listPosts();
  return ["/", "/posts", ...posts.map((post) => `/posts/${post.slug}`)];
}
