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

  /** Options for {@link serve}. */
  export interface ServeOptions {
    /** Address to bind. Defaults to `"0.0.0.0"`. */
    hostname?: string;
    /** `0` (the default) binds an ephemeral port (read it back from `addr`). */
    port?: number;
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
   */
  export function broadcast(
    connections: Iterable<WebSocketConnection>,
    data: string | ArrayBufferLike | ArrayBufferView | Blob,
  ): void;

  const _default: { serve: typeof serve; broadcast: typeof broadcast };
  export default _default;
}
