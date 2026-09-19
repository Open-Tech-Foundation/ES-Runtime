// API route — folder path is the URL (/api/hello). Handlers use standard Request/Response.
// Middleware runs first; read shared data from context.locals.
// https://web.opentechf.org/docs/routing/api-routes
export function GET(_request, context) {
  return Response.json({
    message: "Hello from your API route 👋",
    requestId: context.locals?.requestId ?? null,
    time: new Date().toISOString(),
  });
}