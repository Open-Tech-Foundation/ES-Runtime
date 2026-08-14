/**
 * What renders when a loader or a component throws, and what renders for a URL
 * that matches nothing.
 *
 * Both arrive here because both are the same thing to a router: a route that
 * could not produce a page. Handling them together is what stops a 404 from
 * being the one page in an app nobody styled.
 *
 * # Why the message is not shown in production
 *
 * An unexpected error's message is written for whoever is going to fix it, and
 * it routinely names a database, a hostname or a path. What reaches the browser
 * here is the status and nothing else; the detail goes to the server's log,
 * where the person who can act on it is already looking.
 */
import { useEffect } from "react";
import { Link, isRouteErrorResponse, useRouteError } from "react-router";

import { Masthead } from "./Layout.tsx";

export function ErrorPage() {
  const error = useRouteError();
  const status = isRouteErrorResponse(error) ? error.status : 500;

  // This page names itself rather than going through `handle.meta`, because a
  // route that threw has no loader data for its own `meta` to read. On the
  // server the same title is chosen in `src/render.tsx`; here it covers a
  // client-side navigation that lands on an error.
  useEffect(() => {
    document.title = status === 404 ? "Not found · {{name}}" : "Error · {{name}}";
  }, [status]);

  // A `Response` thrown by a loader — a deliberate 404, a 403, a redirect that
  // did not happen. The status is meaningful and the message is ours.
  //
  // The shell and masthead are rendered here rather than inherited: this
  // component *replaces* the layout route's element, so without them an error
  // page would arrive unstyled and with no navigation on it.
  if (isRouteErrorResponse(error)) {
    return (
      <div className="shell">
        <Masthead />
        <p className="error-code">Error {error.status}</p>
        <h1>{error.status === 404 ? "No such page" : error.statusText || "Something went wrong"}</h1>
        <p className="lede">
          {error.status === 404
            ? "The link may be out of date, or the address may have a typo in it."
            : "That request could not be completed."}
        </p>
        <Link className="button button-primary" to="/">
          Back to the start
        </Link>
      </div>
    );
  }

  // Anything else is a bug: a component threw, a loader rejected, a network
  // call failed. In development the stack is worth more than the tidy page.
  return (
    <div className="shell">
      <Masthead />
      <p className="error-code">Error 500</p>
      <h1>Something went wrong</h1>
      <p className="lede">
        This one is not your fault. The details are in the server log.
      </p>
      {/* esdev replaces this with a literal, so a production build drops the
          branch entirely rather than shipping a stack trace it decides at
          runtime not to show. */}
      {process.env.NODE_ENV !== "production" && error instanceof Error ? (
        <pre>
          <code>{error.stack}</code>
        </pre>
      ) : null}
      <Link className="button button-primary" to="/">
        Back to the start
      </Link>
    </div>
  );
}
