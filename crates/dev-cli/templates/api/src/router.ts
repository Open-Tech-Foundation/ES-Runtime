/**
 * Matching a request to a handler.
 *
 * `URLPattern` does the work, and it is a **web standard this runtime already
 * has** — so a router is a table and a loop rather than a dependency. Path
 * parameters, wildcards and optional segments all come from the platform.
 *
 * ```ts
 * { method: "GET", path: "/tasks/:id", handle: showTask }
 * ```
 *
 * # Why the method is matched separately
 *
 * A path that exists but does not answer this method is a **405**, not a 404 —
 * and the response has to say which methods it does answer. That distinction is
 * lost by a router keyed on `"GET /tasks/:id"`, which is why the two are kept
 * apart here: [`match`] reports "no such path" and "wrong method" as different
 * answers, because they are different answers.
 */

/** What a handler is given: the request, and whatever the path captured. */
export type Context = {
  request: Request;
  params: Record<string, string>;
  url: URL;
};

export type Handler = (context: Context) => Response | Promise<Response>;

export type Route = {
  method: string;
  /** A `URLPattern` pathname: `/tasks`, `/tasks/:id`, `/files/*`. */
  path: string;
  handle: Handler;
};

/** A route table, with its patterns compiled once. */
export class Router {
  private readonly compiled: { route: Route; pattern: URLPattern }[];

  constructor(routes: Route[]) {
    // Compiled here rather than per request: a `URLPattern` is a parsed
    // grammar, and building one for every route on every request is the kind
    // of cost that never shows up in development and always shows up in a
    // load test.
    this.compiled = routes.map((route) => ({
      route,
      pattern: new URLPattern({ pathname: route.path }),
    }));
  }

  /**
   * The handler for a request, or what to say instead.
   *
   * `"not-found"` means no route has this path. `"method-not-allowed"` means
   * one does and answers other methods, which it names — a client that got a
   * 405 without an `Allow` header learns nothing from it.
   */
  match(method: string, pathname: string): Matched {
    // `HEAD` is `GET` without a body. Answering it separately is a second
    // implementation of every route that can disagree with the first; the
    // runtime drops the body for us.
    const wanted = method === "HEAD" ? "GET" : method;
    const allowed = new Set<string>();

    for (const { route, pattern } of this.compiled) {
      const found = pattern.exec({ pathname });
      if (!found) continue;
      if (route.method === wanted) {
        return {
          kind: "found",
          handle: route.handle,
          // `groups` values are `string | undefined` — an optional segment that
          // did not match is absent. Dropping those keeps `params` a plain
          // record a handler can index without checking.
          params: Object.fromEntries(
            Object.entries(found.pathname.groups).filter(
              (entry): entry is [string, string] => entry[1] !== undefined,
            ),
          ),
        };
      }
      allowed.add(route.method);
    }

    if (allowed.size === 0) {
      return { kind: "not-found" };
    }
    // A route that answers GET answers HEAD, and every route answers OPTIONS.
    if (allowed.has("GET")) {
      allowed.add("HEAD");
    }
    allowed.add("OPTIONS");
    return { kind: "method-not-allowed", allowed: [...allowed].sort() };
  }
}

export type Matched =
  | { kind: "found"; handle: Handler; params: Record<string, string> }
  | { kind: "not-found" }
  | { kind: "method-not-allowed"; allowed: string[] };
