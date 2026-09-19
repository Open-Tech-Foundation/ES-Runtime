// Server middleware for every matching request — pages, API routes, loader data, and SSR.
// Stamp shared data on context.locals; API handlers and loaders read it downstream.
// https://web.opentechf.org/docs/routing/middleware
export default async function (request, context, next) {
  context.locals.requestId = crypto.randomUUID();
  return next();
}