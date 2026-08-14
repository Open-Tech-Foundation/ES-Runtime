/**
 * The frame every page renders inside.
 *
 * It is a route — the one at `/`, with the rest as its children — so react-router
 * keeps it mounted across a navigation and only swaps what `<Outlet />` marks.
 */
import { useEffect } from "react";
import { NavLink, Outlet, ScrollRestoration, useMatches, useNavigation } from "react-router";

import { pickMeta } from "../http/head.ts";
import type { Handle } from "../routes.tsx";

/**
 * Keeps `document.title` on the route.
 *
 * The server writes the title into the document it sends, which covers the
 * first page and nothing after it: a client-side navigation never touches
 * `<head>`, so without this the tab keeps the name of whichever page the
 * visitor happened to land on. It reads the same `handle.meta` through the same
 * [`pickMeta`] the server uses, so the two cannot disagree.
 */
function useDocumentTitle() {
  const matches = useMatches();
  useEffect(() => {
    const meta = pickMeta(
      matches.map((match) => ({
        meta: (match.handle as Handle | undefined)?.meta,
        data: match.loaderData,
      })),
      { title: "{{name}}" },
    );
    document.title = meta.title;
  }, [matches]);
}

/**
 * The masthead, shared by [`Layout`] and [`ErrorPage`].
 *
 * Shared because an error page that loses the site's navigation is a dead end:
 * a route's ErrorBoundary replaces *its own* route's element, and the boundary
 * here is on the layout — so without this, a 404 would render with no header
 * and no way back except the browser's Back button.
 */
export function Masthead() {
  return (
    <header className="masthead">
      <NavLink to="/" className="wordmark" end>
        {"{{name}}"}
      </NavLink>
      <nav>
        <NavLink to="/" end>
          Home
        </NavLink>
        <NavLink to="/posts">Writing</NavLink>
      </nav>
    </header>
  );
}

export function Layout() {
  const navigation = useNavigation();
  useDocumentTitle();

  return (
    // A slow loader is otherwise invisible: the old page stays put with no sign
    // that anything is happening. This is the whole of that feedback.
    <div className={navigation.state === "loading" ? "navigating" : undefined}>
      <div className="shell">
        <Masthead />

        <main>
          <Outlet />
        </main>
      </div>

      {/* Restores the scroll position on Back, which a browser does for free on
          a full page load and not at all once a router takes over navigation. */}
      <ScrollRestoration />
    </div>
  );
}
