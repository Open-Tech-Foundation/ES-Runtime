/**
 * The route table — the one description of this app that everything reads.
 *
 * The server matches a request against it, the browser hydrates from it, and
 * the prerender step walks it to know what pages exist. Keeping it in one
 * module is what makes those three agree; a route that only the server knows
 * about is a 404 in the browser, and nothing would report it.
 *
 * # What a route may carry
 *
 * - `loader` — runs **before** the component renders, on whichever side is
 *   doing the rendering. Its result reaches the browser with the document, so a
 *   hydrating page does not fetch what the server already had.
 * - `Component` — what to render. `data` comes from `useLoaderData`, typed.
 * - `ErrorBoundary` — what to render when a loader or a component below this
 *   point throws. It replaces *its own* route's element, so the boundary on the
 *   layout below replaces the layout: an error renders a bare page, not a page
 *   inside the masthead. Move it onto the children to keep the frame around it.
 * - `handle.meta` — the `<title>` and `<meta>` for this route, derived from its
 *   own data. See [`src/document.ts`].
 */
import type { RouteObject } from "react-router";

import { Layout } from "./app/Layout.tsx";
import { ErrorPage } from "./app/ErrorPage.tsx";
import { Home } from "./app/Home.tsx";
import { Posts } from "./app/Posts.tsx";
import { Post } from "./app/Post.tsx";
import { findPost, listPosts, type Post as PostData } from "./data/posts.ts";
import type { Meta } from "./http/head.ts";

export type { Meta };

/**
 * The extra field this app puts on a route.
 *
 * `handle` is react-router's escape hatch — it carries anything, and hands it
 * back on the matched route. It is how a route describes its own `<title>`
 * without a second table to keep in sync with this one.
 */
export type Handle = {
  /**
   * This route's `<title>` and `<meta>`, from its own loader data.
   *
   * **`data` may be `undefined`**, and a `meta` that ignores that will throw:
   * the browser asks again on every navigation, including while a loader is
   * still resolving and including one that ended in an error. Read it
   * defensively and return a sensible title for the moment before the real one
   * is knowable — `posts/:slug` below is the worked example.
   */
  meta?: (data: unknown) => Meta;
};

export const routes: RouteObject[] = [
  {
    path: "/",
    Component: Layout,
    // On the layout rather than on each child, so one boundary covers every
    // route below — including a URL that matches nothing, which react-router
    // reports here as a 404.
    ErrorBoundary: ErrorPage,
    children: [
      {
        index: true,
        Component: Home,
        handle: {
          meta: () => ({
            title: "{{name}}",
            description: "A React app on the ES Runtime: server-rendered, hydrated, prerenderable.",
          }),
        } satisfies Handle,
      },
      {
        path: "posts",
        Component: Posts,
        loader: async () => ({ posts: await listPosts() }),
        handle: {
          meta: () => ({ title: "Writing · {{name}}" }),
        } satisfies Handle,
      },
      {
        // A dynamic segment. `params.slug` is typed as a string by
        // react-router, and the loader is where it stops being one.
        path: "posts/:slug",
        Component: Post,
        loader: async ({ params }) => {
          const post = await findPost(params.slug!);
          if (!post) {
            // A thrown Response is how a loader says "this is not a page". The
            // status reaches the browser as a real 404 — see src/server.tsx —
            // and ErrorBoundary renders it.
            throw new Response("Not Found", { status: 404 });
          }
          return { post };
        },
        handle: {
          // `data` is the loader's result — but it is not always there when
          // this is asked. The browser calls it again on every navigation,
          // including the moment a route is matched and its loader has not
          // resolved, and including a navigation that ended in an error. So it
          // is written to cope rather than to assume; the fallback is what the
          // tab reads for the instant before the real title arrives.
          meta: (data) => {
            const post = (data as { post?: PostData } | undefined)?.post;
            return post
              ? { title: `${post.title} · {{name}}`, description: post.summary }
              : { title: "{{name}}" };
          },
        } satisfies Handle,
      },
    ],
  },
];
