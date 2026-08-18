/**
 * What renders when a loader or a component throws, and what renders for a URL
 * that matches nothing — both are the same thing to a router: a route that
 * could not produce a page.
 *
 * It *replaces* the layout route's element, so it renders its own shell.
 *
 * An unexpected error's message is written for whoever is going to fix it and
 * routinely names a hostname or a path, so what reaches the browser in
 * production is the status and nothing else; the detail is in the server's log.
 */
import { useEffect } from "react";
import { Link, isRouteErrorResponse, useRouteError } from "react-router";

export function ErrorPage() {
  const error = useRouteError();
  const status = isRouteErrorResponse(error) ? error.status : 500;

  // This page names itself rather than going through `handle.meta`: a route
  // that threw has no loader data for its own `meta` to read. The server
  // chooses the same title in src/render.tsx.
  useEffect(() => {
    document.title = status === 404 ? "Not found · {{name}}" : "Error · {{name}}";
  }, [status]);

  return (
    <main className="shell">
      <h1>{status === 404 ? "No such page" : "Something went wrong"}</h1>
      <p className="lede">
        {status === 404
          ? "Nothing in src/routes.tsx matches this address."
          : "The details are in the server log."}
      </p>
      {/* esdev replaces this with a literal, so a production build drops the
          branch entirely rather than shipping a stack trace it decides at
          runtime not to show. */}
      {process.env.NODE_ENV !== "production" && error instanceof Error ? (
        <pre>
          <code>{error.stack}</code>
        </pre>
      ) : null}
      <p>
        <Link to="/">Back to the start</Link>
      </p>
    </main>
  );
}
