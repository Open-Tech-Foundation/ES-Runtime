/**
 * The route table — the one description of this app that everything reads. The
 * server matches a request against it, the browser hydrates from it, and the
 * static build walks it to know what pages exist.
 *
 * A route may carry a `loader` (runs before the component renders, on whichever
 * side is rendering), a `Component`, an `ErrorBoundary`, and `handle.meta` —
 * this page's `<title>` and `<meta>`, from its own data.
 */
import type { RouteObject } from "react-router";

import { ErrorPage } from "./app/ErrorPage.tsx";
import { Home } from "./app/Home.tsx";
import { Layout } from "./app/Layout.tsx";
import type { Meta } from "./http/head.ts";

export type { Meta };

/**
 * The extra field this app puts on a route: what to call the page.
 *
 * `handle` is react-router's escape hatch — it carries anything, and hands it
 * back on the matched route. It is how a route describes its own `<title>`
 * without a second table to keep in sync with this one.
 */
export type Handle = {
  /**
   * This route's `<title>` and `<meta>`, from its loader data — which **may be
   * `undefined`**, since the browser asks again on every navigation, including
   * while a loader is still resolving. Read it defensively.
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
            description: "Built with ES Runtime, from the Open Tech Foundation.",
          }),
        } satisfies Handle,
      },
    ],
  },
];
