/**
 * One render — shared by the server and the prerender step, so a page cannot
 * come out one way live and another way static.
 *
 * `createStaticHandler(routes).query(request)` runs the matched route's loader
 * and hands back the data, the matches and the status they imply;
 * `<StaticRouterProvider>` renders that, and emits the `<script>` carrying the
 * same data to the browser — escaping included, which is why nothing here
 * serialises it by hand.
 */
import { renderToReadableStream } from "react-dom/server.browser";
import {
  StaticRouterProvider,
  createStaticHandler,
  createStaticRouter,
  type StaticHandlerContext,
} from "react-router";

import { document } from "./document.ts";
import { head, pickMeta } from "./http/head.ts";
import { type Handle, type Meta, routes } from "./routes.tsx";

/** Built once. It reads the route table and holds no per-request state. */
const handler = createStaticHandler(routes);

const encoder = new TextEncoder();

export type Rendered = {
  body: ReadableStream<Uint8Array>;
  status: number;
  /** Resolves when every byte has been produced — see [`render`]. */
  allReady: Promise<void>;
};

/**
 * Renders `request` into a complete HTML document.
 *
 * Returns a stream that has already begun: the head is written before React is
 * asked for anything, so the browser can start fetching the stylesheet and the
 * bundle while the server is still rendering.
 */
export async function render(request: Request, scriptNonce: string): Promise<Rendered | Response> {
  const context = await handler.query(request);

  // A loader that returned a `redirect()` produces a Response rather than a
  // context. It is already the answer; rendering a page around it would throw
  // away the status and the Location header both.
  if (context instanceof Response) {
    return context;
  }

  const router = createStaticRouter(routes, context);

  const app = await renderToReadableStream(
    <StaticRouterProvider router={router} context={context} nonce={scriptNonce} />,
    {
      nonce: scriptNonce,
      // Errors thrown *after* the shell has been sent cannot change the status
      // — the headers are long gone. React recovers by rendering the route's
      // ErrorBoundary into the stream; this is only so the server can log it.
      onError(error) {
        console.error("render:", error);
      },
      // Stops the render when the client goes away mid-response, instead of
      // finishing a page nobody is waiting for.
      signal: request.signal,
    },
  );

  // `renderToReadableStream` resolves once the shell is ready, so anything
  // inside a `<Suspense>` boundary is still being worked on and arrives on this
  // stream as it completes.
  const meta = metaFor(context);
  const body = new ReadableStream<Uint8Array>({
    async start(controller) {
      controller.enqueue(encoder.encode(document.beforeHead));
      controller.enqueue(encoder.encode(head(meta)));
      controller.enqueue(encoder.encode(document.beforeApp));

      // A reader rather than `for await`: async iteration over a
      // `ReadableStream` is not in every engine, and React's own stream type
      // does not declare it. This is the spelling that works everywhere.
      const reader = app.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        controller.enqueue(value);
      }

      controller.enqueue(encoder.encode(document.afterApp));
      controller.close();
    },
    cancel(reason) {
      void app.cancel(reason);
    },
  });

  return { body, status: context.statusCode, allReady: app.allReady };
}

/**
 * The head for whatever matched, from the deepest route that describes one.
 *
 * Deepest-first so a specific route wins over the layout it sits in, which is
 * the same way every other nested thing in a router resolves.
 */
function metaFor(context: StaticHandlerContext): Meta {
  // A route that threw has no loader data, and what renders is its
  // ErrorBoundary rather than its component — so its own `meta`, which is
  // written against data that never arrived, must not be called. The title
  // describes the page that is actually being sent.
  if (context.errors) {
    return {
      title: context.statusCode === 404 ? "Not found · {{name}}" : "Error · {{name}}",
    };
  }

  return pickMeta(
    context.matches.map((match) => ({
      meta: (match.route.handle as Handle | undefined)?.meta,
      data: context.loaderData[match.route.id!],
    })),
    { title: "{{name}}" },
  );
}
