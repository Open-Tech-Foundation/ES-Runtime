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
   *
   * The second argument describes the connection the request arrived on. It is
   * optional to take — a one-parameter handler is unaffected.
   */
  export type Handler = (
    request: Request,
    info: ConnectionInfo,
  ) => Response | Promise<Response>;

  /** What the handler is told about the connection a request arrived on. */
  export interface ConnectionInfo {
    /**
     * The other end of the socket, or `null` when the host has no peer to
     * report (a mock provider, a transport with no address).
     *
     * This is the **socket** peer and only ever that: behind a reverse proxy it
     * is the proxy. `X-Forwarded-For` is never consulted — resolving it takes
     * knowing which hop to trust, and a header anyone can send is not an
     * identity. The header is delivered untouched in `request.headers`, so a
     * deployment that does know can resolve it itself.
     *
     * On HTTP/2 every request multiplexed onto one connection reports the same
     * peer, because they are one connection.
     */
    remoteAddr: NetAddr | null;
  }

  /** A transport address, in the shape `Deno.NetAddr` uses. */
  export interface NetAddr {
    transport: "tcp";
    /** The peer's IP address, e.g. `"203.0.113.7"`. */
    hostname: string;
    /** The peer's port — ephemeral for a client, so rarely meaningful alone. */
    port: number;
  }

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
    /**
     * The most connections to serve at once. Unlimited by default.
     *
     * A connection over the cap is **held, not refused**: the server stops
     * accepting, so it waits in the kernel's backlog and is served as soon as a
     * slot frees. Nothing is spent on it in the meantime — no descriptor, no
     * task, no read buffer.
     *
     * There is no default because the right number follows from the
     * deployment's file-descriptor budget and the memory a connection costs.
     * Worth setting on a public port: an HTTP/1.1 connection's read buffer can
     * reach ~408KB, so the connection count is a memory multiplier.
     */
    maxConnections?: number | null;

    /**
     * Bind with `SO_REUSEPORT`, so several **processes** can listen on this
     * same address and the kernel balances new connections across them.
     *
     * How a server is run across cores without a front proxy, and how one is
     * replaced without dropping connections: the replacement binds alongside
     * the outgoing process before it exits. Every sharer must set it — a plain
     * bind on a port already held is still `ERR_ADDRESS_IN_USE`.
     *
     * **Unix only.** Windows has no equivalent, so asking for it there is an
     * error rather than a silent exclusive bind.
     *
     * @defaultValue `false`
     */
    reusePort?: boolean;
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
    /**
     * How long a request **body** may take, before the allowance
     * {@link bodyMinRate} earns it. Defaults to `30_000`; `null` removes the
     * bound.
     *
     * {@link headerRead} stops when the head is complete, so a peer that sends
     * a well-formed head and then dribbles its body a byte at a time is past
     * every other timer here. A flat cap cannot answer that — over elapsed time
     * a large upload on a slow link looks the same — so the deadline is
     * **earned**: a body starts with this and gains more by arriving.
     */
    bodyRead?: number | null;
    /**
     * Bytes per second that extend {@link bodyRead} — a floor to beat, not a
     * rate to sustain. Defaults to `1024`.
     *
     * The deadline is `bodyRead + received / bodyMinRate`, so at the defaults a
     * 100 MiB upload has over a day to arrive and a peer sending one byte a
     * minute is closed at ~30s. `0` earns nothing, making {@link bodyRead} a
     * flat cap on the whole body.
     */
    bodyMinRate?: number | null;
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
  /**
   * Sends `trailers` after the response body — the header fields that cannot be
   * known until the body has been produced, which is where gRPC carries the
   * status of a call. Returns the same `Response`.
   *
   * Trailers are **not part of the Fetch API** and no runtime exposes them
   * there, which is why this is an import rather than an option on `Response`:
   * the dependency is visible instead of silently doing nothing elsewhere.
   *
   * On HTTP/2 they become a trailing `HEADERS` frame. On HTTP/1.1 the wire
   * format carries only the fields named in the response's `Trailer` header —
   * that header is added for you whenever the names are known before the head
   * goes out, which is everything except a promise attached to a *streaming*
   * body. Declare `Trailer` yourself in that case.
   */
  export function withTrailers(
    response: Response,
    trailers: HeadersInit | Promise<HeadersInit>,
  ): Response;

  /**
   * The header fields that arrived after a response's body.
   *
   * Resolves once the body has been read to its end, because that is when
   * trailers are on the wire — and to an empty `Headers` for a response with
   * none, one built locally, or a body that was cancelled rather than read. It
   * never waits forever on a body nobody is reading.
   */
  export function trailersOf(response: Response): Promise<Headers>;

  export function serve(handler: Handler): Server;
  export function serve(options: ServeOptions, handler: Handler): Server;

  const http: {
    serve: typeof serve;
    withTrailers: typeof withTrailers;
    trailersOf: typeof trailersOf;
  };
  export default http;
}
