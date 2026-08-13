import type { Route, RouteData } from "./routes.ts";

/**
 * The whole page below `<body>` — the same tree on the server, in the browser
 * and in a prerendered file.
 *
 * Navigation is plain `<a href>`: a full page load. A client-side router is a
 * choice this template leaves to you, and one whose absence is invisible on a
 * fast server.
 */
export function App({ route, data }: { route: Route; data: RouteData }) {
  const { Component } = route;
  return (
    <main>
      <nav>
        <a href="/">Home</a>
        <a href="/about">About</a>
      </nav>
      <Component data={data} />
    </main>
  );
}
