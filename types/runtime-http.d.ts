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
    /**
     * When to give up on a connection that is not making progress.
     *
     * These bound only connections that are **idle or stalled**: a request in
     * flight, a body still arriving, and a response still streaming are never
     * interrupted, however long they take.
     */
    timeouts?: ServeTimeouts;
  }

  /**
   * Per-connection deadlines for {@link ServeOptions.timeouts}. Each is a
   * number of milliseconds; `null` disables that one; omitting it keeps the
   * default.
   */
  export interface ServeTimeouts {
    /**
     * From accept until the connection can carry requests: the TLS handshake,
     * and the wait for the first byte the HTTP version is read from. A TLS
     * connection passes both stages, so it may take up to twice this before it
     * counts as established. Defaults to `10_000`.
     */
    handshake?: number | null;
    /**
     * How long a request head may take to arrive in full.
     *
     * On HTTP/1.1 this is **also the idle keep-alive limit**, because waiting
     * for the next request on a kept-alive connection is waiting for a request
     * head: an idle connection is closed after this long and a client that
     * wants another request opens a new one. HTTP/2 keeps its connections open
     * and uses {@link h2KeepAlive} instead. Defaults to `30_000`.
     */
    headerRead?: number | null;
    /**
     * How often an idle HTTP/2 connection is probed with a PING, and how long
     * the ACK may take before it is dropped — so a dead peer is reclaimed
     * within twice this. HTTP/2 connections are long-lived by design and have
     * no idle limit, so without probing a peer that vanishes without a FIN
     * keeps its connection until the OS TCP keepalive notices. Defaults to
     * `20_000`.
     */
    h2KeepAlive?: number | null;
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
   * Start an HTTP/1.1 + HTTP/2 server (capability: `NetListen`). Returns
   * immediately. The version is negotiated per connection — ALPN over TLS, the
   * HTTP/2 preface (h2c) on a cleartext port — and never reaches the handler.
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
