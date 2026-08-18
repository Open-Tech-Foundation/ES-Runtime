/**
 * Responses, and the one error type that becomes one.
 *
 * A handler that threads an error value back through every call it makes ends
 * up checking for it more often than it does anything else, so a failure is
 * thrown and turned into a response in one place — `toResponse`.
 *
 * The rule that makes that safe: **only an `HttpError` carries a message to the
 * client.** Anything else is a bug, and a bug's message names hostnames, paths
 * and sometimes the data itself. Those get a flat 500, and the detail stays in
 * the log where whoever can act on it is already looking.
 */

/** A failure with a status the client should be told about. */
export class HttpError extends Error {
  readonly status: number;
  /** Field-level detail, so a client can point at what was wrong. */
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
