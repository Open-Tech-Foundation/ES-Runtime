declare module "runtime:websocket" {
  /**
   * A server-side WebSocket connection: an already-open socket, so there is no
   * connecting handshake to observe. Otherwise the same surface as the global
   * `WebSocket` client.
   */
  export interface WebSocketConnection {
    /** How binary messages are delivered. Defaults to `"blob"`. */
    binaryType: "blob" | "arraybuffer";
    /** Send a message to this peer. */
    send(data: string | ArrayBufferLike | ArrayBufferView | Blob): void;
    /**
     * Bytes handed to {@link send} that the host has not taken yet — the only
     * way a sender can feel a peer that has stopped reading, since `send()` is
     * fire-and-forget and never stalls.
     *
     * A number that keeps climbing is a peer to stop sending to. Past the
     * server's `maxBufferedAmount` the host closes the connection with `1013`
     * rather than hold more, so ignoring this costs the connection, not the
     * process.
     *
     * ```js
     * for await (const chunk of source) {
     *   if (conn.bufferedAmount > 1 << 20) break; // this peer is behind
     *   conn.send(chunk);
     * }
     * ```
     */
    readonly bufferedAmount: number;
    /** Close the connection with an optional code and reason. */
    close(code?: number, reason?: string): void;

    /** Called for each inbound message. */
    onmessage: ((event: MessageEvent) => void) | null;
    /** Called once the connection has closed. */
    onclose: ((event: CloseEvent) => void) | null;
    /** Called if the connection fails. */
    onerror: ((event: Event) => void) | null;

    addEventListener(
      type: "message" | "close" | "error",
      listener: (event: never) => void,
      options?: boolean | AddEventListenerOptions,
    ): void;
    removeEventListener(
      type: "message" | "close" | "error",
      listener: (event: never) => void,
      options?: boolean | EventListenerOptions,
    ): void;
  }

  /** When to give up on a connection that is not making progress. */
  export interface ServeTimeouts {
    /**
     * From accept until the opening handshake completes, in milliseconds.
     * `null` disables it.
     *
     * RFC 6455's handshake is an HTTP request head and a `101` answer, so this
     * is the slowloris bound — a peer that opens a connection and never sends
     * its upgrade request otherwise holds a task and a file descriptor for as
     * long as it likes, at no cost to itself.
     *
     * It does **not** bound an established connection. A WebSocket that has
     * said nothing for a week is idle, not stalled, and closing it is your
     * application's decision.
     *
     * Default: `10000`.
     */
    handshake?: number | null;
  }

  /** Options for {@link serve}. */
  export interface ServeOptions {
    /** Address to bind. Defaults to `"0.0.0.0"`. */
    hostname?: string;
    /** `0` (the default) binds an ephemeral port (read it back from `addr`). */
    port?: number;
    /**
     * When to give up on a connection that is not making progress. Same shape
     * and defaults as `runtime:http`'s `serve({ timeouts })`.
     */
    timeouts?: ServeTimeouts;
    /**
     * The most connections to hold at once. Unlimited by default.
     *
     * A connection over the cap is **held, not refused**: it waits in the
     * kernel's backlog and is served once a slot frees, costing this server
     * nothing in the meantime — no descriptor, no task, no buffers.
     *
     * Worth setting on a public port. WebSocket connections are long-lived by
     * design, so unlike an HTTP server's, the count does not fall back down on
     * its own — this is what decides whether it has an upper bound at all. The
     * right number follows from your file-descriptor budget, which the runtime
     * cannot read.
     */
    maxConnections?: number | null;

    /**
     * The most connections **one peer address** may hold at once. No limit by
     * default.
     *
     * {@link maxConnections} bounds what the deployment spends and nothing
     * else: one peer opening every slot fills the server exactly as a thousand
     * peers opening one each do, and it is then full for everybody. This is the
     * half that says *whose* connections they are.
     *
     * A connection over this is **refused**, where one over
     * {@link maxConnections} waits — an excess there is legitimate traffic
     * queueing for a slot, and an excess here is one client past its share.
     *
     * Off by default for a sharper reason than {@link maxConnections}: the
     * count is per address, so everything behind one NAT or one load balancer
     * shares a budget. **With a proxy in front, every connection has the same
     * source address and any cap here caps the whole service** — leave it off
     * and use the proxy's own limits.
     */
    maxConnectionsPerIp?: number | null;
    /**
     * The most bytes that may sit queued for one connection before it is closed
     * with `1013` (Try Again Later). Defaults to `8_388_608` (8 MiB); `0`
     * removes the bound.
     *
     * `send()` is fire-and-forget — the WebSocket API has no way to report a
     * full buffer — so writing faster than a peer reads never stalls your code;
     * the messages queue on the host instead, one pending send each. A peer that
     * stops reading a fan-out is otherwise a memory leak with a network
     * interface.
     *
     * Unlike the connection caps this is **on** by default, because the number
     * does not depend on what the deployment knows: a queue that deep is already
     * a peer several messages behind, and no application intends one. Read
     * {@link WebSocketConnection.bufferedAmount} to pace sends and stay under
     * it.
     */
    maxBufferedAmount?: number | null;
  }

  /**
   * A running WebSocket server: async-iterable over accepted connections.
   *
   * ```js
   * for await (const ws of serve({ port: 4001 })) { … }
   * ```
   */
  export interface WebSocketServer
    extends AsyncIterable<WebSocketConnection> {
    /** The bound address (resolves once the server is listening). */
    readonly addr: Promise<{ hostname: string; port: number }>;
    /** The next accepted connection, or `null` once the server is closed. */
    accept(): Promise<WebSocketConnection | null>;
    /** Stop accepting and shut the server down. */
    close(): Promise<void>;
  }

  /**
   * Bind a WebSocket server. Requires the `NetListen` capability.
   *
   * `ws:` only — a `wss:` server is a follow-up; terminate TLS at a proxy.
   */
  export function serve(options?: ServeOptions): WebSocketServer;

  /**
   * Send one message to many connections in a single host crossing.
   *
   * Prefer this to a `send()` loop for chat-style fan-out: it marshals the
   * payload once for the whole room, enqueues to every connection concurrently
   * (so one slow peer cannot stall the rest), and coalesces the writes.
   *
   * A **closed** connection is skipped — keeping one in the room's set until its
   * close handler removes it is ordinary. An element that is not a connection at
   * all throws a `TypeError`, checked across the whole iterable before anything
   * is sent, so a bad element fails the call rather than half-delivering it.
   */
  export function broadcast(
    connections: Iterable<WebSocketConnection>,
    data: string | ArrayBufferLike | ArrayBufferView | Blob,
  ): void;

  const _default: { serve: typeof serve; broadcast: typeof broadcast };
  export default _default;
}
