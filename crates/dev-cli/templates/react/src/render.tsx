/**
 * One render, as a stream — shared by the server and the prerender step, so a
 * page cannot come out one way live and another way static.
 */
import { renderToReadableStream } from "react-dom/server.browser";
import { App } from "./app/App.tsx";
import { document, serialize } from "./document.ts";
import type { Route, RouteData } from "./app/routes.ts";

const encoder = new TextEncoder();

/** The whole document for `route`, streamed as it renders. */
export async function render(route: Route, data: RouteData): Promise<ReadableStream<Uint8Array>> {
  const app = await renderToReadableStream(<App route={route} data={data} />);
  return new ReadableStream({
    async start(controller) {
      // The head is on the wire before React has rendered anything, so the
      // browser can start fetching the stylesheet and the bundle while the
      // server is still working.
      controller.enqueue(encoder.encode(document.beforeApp));
      for await (const chunk of app) controller.enqueue(chunk);
      controller.enqueue(encoder.encode(document.afterApp));
      controller.enqueue(encoder.encode(serialize(data)));
      controller.enqueue(encoder.encode(document.afterData));
      controller.close();
    },
  });
}
