/**
 * Responses, and the one error type that becomes one.
 *
 * # Errors are thrown, not returned
 *
 * A handler that has to thread an error value back through every call it makes
 * ends up checking for it more often than it does anything else. Throwing an
 * [`HttpError`] lets the failure travel from wherever it was noticed to the one
 * place that turns it into a response, and every handler in between stays about
 * what it is for.
 *
 * The rule that makes that safe is that **only an `HttpError` carries a message
 * to the client**. Anything else is a bug, and a bug's message is written for
 * whoever will fix it: it names hostnames, paths, query fragments and sometimes
 * the data itself. [`toResponse`] gives those a flat 500 and leaves the detail
 * in the log, where the person who can act on it is already looking.
 */

/** A failure with a status the client should be told about. */
export class HttpError extends Error {
  readonly status: number;
  /** Field-level detail for a 422, so a form can point at what was wrong. */
  readonly details?: Record<string, string>;

  constructor(status: number, message: string, details?: Record<string, string>) {
    super(message);
    this.name = "HttpError";
    this.status = status;
    this.details = details;
  }

  static notFound(what = "Not found") {
    return new HttpError(404, what);
  }

  static badRequest(message: string, details?: Record<string, string>) {
    return new HttpError(400, message, details);
  }

  /** 422: the request parsed, and what it said cannot be acted on. */
  static invalid(details: Record<string, string>) {
    return new HttpError(422, "Validation failed", details);
  }
}

/** A JSON response. */
export function json(body: unknown, init: ResponseInit = {}): Response {
  return new Response(JSON.stringify(body), {
    ...init,
    headers: {
      "content-type": "application/json; charset=utf-8",
      ...securityHeaders(),
      ...init.headers,
    },
  });
}

/** `204`, for something that succeeded and has nothing to say. */
export function noContent(): Response {
  return new Response(null, { status: 204, headers: securityHeaders() });
}

/**
 * Whatever was thrown, as a response.
 *
 * Returns the response *and* whether it was unexpected, so the caller can log
 * the ones that are bugs and stay quiet about the ones that are ordinary — a
 * log full of "404 Not found" hides the one line that mattered.
 */
export function toResponse(error: unknown): { response: Response; unexpected: boolean } {
  if (error instanceof HttpError) {
    return {
      response: json(
        { error: error.message, ...(error.details ? { details: error.details } : {}) },
        { status: error.status },
      ),
      unexpected: false,
    };
  }
  return {
    response: json({ error: "Internal Server Error" }, { status: 500 }),
    unexpected: true,
  };
}

/**
 * The headers every response carries.
 *
 * Short, because an API serves JSON rather than documents: there is no markup
 * for a CSP to govern and nothing to frame. `nosniff` is the one that matters —
 * without it a browser may decide a JSON response full of user-supplied text is
 * HTML, and run it.
 */
export function securityHeaders(): Record<string, string> {
  return {
    "x-content-type-options": "nosniff",
    // An API answers programs, not links. There is no Referer worth leaking.
    "referrer-policy": "no-referrer",
    // Nothing here is a page, so nothing here should ever be framed.
    "content-security-policy": "default-src 'none'; frame-ancestors 'none'",
  };
}

/**
 * The request's JSON body, or a 400 explaining why not.
 *
 * The content type is checked rather than assumed: a form post arriving at a
 * JSON endpoint should be told what is wrong, not fail on a parse error that
 * reads like a server bug.
 */
export async function readJson(request: Request): Promise<unknown> {
  const type = request.headers.get("content-type") ?? "";
  if (!type.toLowerCase().includes("application/json")) {
    throw HttpError.badRequest("Expected a JSON body (content-type: application/json)");
  }
  try {
    return await request.json();
  } catch {
    // The parser's own message names an offset in bytes the client cannot see.
    throw HttpError.badRequest("The request body is not valid JSON");
  }
}
