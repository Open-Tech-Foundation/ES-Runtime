import type { MiddlewareContext, NextFn } from "@opentf/afw/server";

// Applies to every request under `/` — pages, API routes, loader data, 404s.
// `context.locals` carries values down to API handlers and loaders;
// returning a response instead of `next(...)` short-circuits the pipeline.
export default async function middleware(
  request: Request,
  context: MiddlewareContext,
  next: NextFn,
) {
  const response = await next(request);
  response.headers.set("x-powered-by", "afw");
  return response;
}
