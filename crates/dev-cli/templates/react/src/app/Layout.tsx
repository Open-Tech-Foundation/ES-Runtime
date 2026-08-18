/**
 * The frame every page renders inside.
 *
 * It is a route — the one at `/`, with the rest as its children — so
 * react-router keeps it mounted across a navigation and only swaps what
 * `<Outlet />` marks.
 */
import { useEffect } from "react";
import { Outlet, useMatches } from "react-router";

import { pickMeta } from "../http/head.ts";
import type { Handle } from "../routes.tsx";

/**
 * Keeps `document.title` on the route.
 *
 * The server writes the title into the document it sends, which covers the
 * first page and nothing after it: a client-side navigation never touches
 * `<head>`. It reads the same `handle.meta` through the same `pickMeta` the
 * server uses, so the two cannot disagree.
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

export function Layout() {
  useDocumentTitle();

  return (
    <main className="shell">
      <Outlet />
    </main>
  );
}
