/**
 * A response with its body dropped, for a `HEAD` request.
 *
 * HEAD is GET without the body: same status, same headers, and nothing after
 * them. The server already holds that up — a body handed to it for a HEAD is
 * not written to the wire — so this is not what stops a client waiting. What it
 * does is make the response *be* what is sent, which buys two things.
 *
 * **The stream is closed rather than dropped.** An asset response is an open
 * file being read; handing it to a server that will not read it leaves the
 * handle alive until the collector gets to it. Cancelling gives it back now.
 *
 * **A body nobody will send is not produced.** Rendering a page is work, and on
 * a HEAD it is work whose entire output is discarded a layer down.
 *
 * Done here, once, over whatever a route returned, rather than in each handler:
 * the rule is about the *method*, not about the page, and a handler that has to
 * remember it is a handler that will eventually forget.
 *
 * The headers are kept exactly as the handler set them — RFC 9110 §9.3.2 asks
 * for the ones GET would have sent, so a client can learn a resource's type and
 * size without fetching it, which is the only reason to send a HEAD at all.
 * Framing is the server's: whether a response goes out with `content-length` or
 * `transfer-encoding` is decided below this, and a HEAD carries neither because
 * there is no body to frame.
 */
export function withoutBody(response: Response): Response {
  if (!response.body) {
    return response;
  }

  // Cancelled rather than left alone: the body of an asset response is an open
  // file being streamed, and dropping the reference without cancelling leaks the
  // handle until the collector gets to it.
  void response.body.cancel();

  return new Response(null, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}
