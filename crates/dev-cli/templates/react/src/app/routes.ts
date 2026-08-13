/**
 * The route table.
 *
 * A table rather than a router dependency: what a route needs here is a path,
 * something to render, and — for the server — something to fetch first. Three
 * fields is not a library, and keeping it as data means the same table is read
 * by the server (to render), by the client (to hydrate) and by the prerender
 * step (to know what pages exist).
 */
import type { ComponentType } from "react";
import { Home } from "./Home.tsx";
import { About } from "./About.tsx";

export type RouteData = { title: string; body: string };

export type Route = {
  path: string;
  Component: ComponentType<{ data: RouteData }>;
  /** Runs on the server, before rendering. Its result reaches the browser. */
  loader: () => Promise<RouteData> | RouteData;
};

export const routes: Route[] = [
  {
    path: "/",
    Component: Home,
    loader: () => ({
      title: "It works",
      body: "Rendered on the server, hydrated in the browser.",
    }),
  },
  {
    path: "/about",
    Component: About,
    loader: () => ({
      title: "About",
      body: "The same components render here, on the server and in the browser.",
    }),
  },
];

/** The route for a path, or `undefined` if nothing matches. */
export function match(pathname: string): Route | undefined {
  const path = pathname.length > 1 ? pathname.replace(/\/$/, "") : pathname;
  return routes.find((route) => route.path === path);
}
