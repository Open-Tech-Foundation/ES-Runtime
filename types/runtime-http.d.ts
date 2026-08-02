declare module "runtime:http" {
  /**
   * An HTTP request handler: called with a web `Request`, returns (or resolves
   * to) a web `Response`. A thrown error or a non-`Response` return becomes a
   * `500`. Bodies stream: the request body is a `ReadableStream` pulling chunks
   * as they arrive, and a `ReadableStream` response body is sent with chunked
   * transfer-encoding as it is produced (`new Response(request.body)` proxies
   * without buffering).
   *
   * `request.signal` aborts if the client disconnects before the handler has
   * produced a response, so work nobody will read can be abandoned — pass it
   * straight to `fetch` or anything else taking a signal. Reading it is what
   * starts the watch, so a handler that never asks pays nothing.
   *
   * The HTTP version is negotiated per connection (HTTP/1.1 or HTTP/2) and is
   * not visible here: the same `Request` arrives either way.
   */
  export type Handler = (request: Request) => Response | Promise<Response>;

  /** Options for {@link serve}. */
  export interface ServeOptions {
    /** Address to bind. Defaults to `"0.0.0.0"`. */
    hostname?: string;
    host?: string;
    /** `0` (the default) binds an ephemeral port (read it back from `addr`). */
    port?: number;
    /**
     * `"on"` terminates TLS on accept — requires {@link cert} and {@link key}.
     * Defaults to `"off"` (plain HTTP).
     */
    secureTransport?: "on" | "off";
    /** PEM certificate chain, leaf first. Required when `secureTransport` is `"on"`. */
    cert?: string | Uint8Array;
    /** PEM private key. Required when `secureTransport` is `"on"`. */
    key?: string | Uint8Array;
    /**
     * ALPN protocols to advertise. Defaults to `["h2", "http/1.1"]` — the
     * server speaks both, and the client picks. Narrow it to pin a version,
     * e.g. `["http/1.1"]`.
     */
    alpn?: string[];
  }

  /** A running HTTP server. */
  export interface Server {
    /** The bound address (resolves once the server is listening). */
    readonly addr: Promise<{ hostname: string; port: number }>;
    /** Resolves when the accept loop has ended (after {@link stop}). */
    readonly finished: Promise<void>;
    /** Stop accepting and shut the server down; resolves once stopped. */
    stop(): Promise<void>;
  }

  /**
   * Start an HTTP/1.1 server (capability: `NetListen`). Returns immediately.
   *
   * With `secureTransport: "on"` it serves HTTPS, and `request.url` reports the
   * `https:` scheme. The cert and key are passed inline rather than as paths —
   * reading a file is the filesystem's privilege — so serving HTTPS needs no
   * grant beyond `NetListen`.
   */
  export function serve(handler: Handler): Server;
  export function serve(options: ServeOptions, handler: Handler): Server;

  const http: { serve: typeof serve };
  export default http;
}
