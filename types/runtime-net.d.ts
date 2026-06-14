declare module "runtime:net" {
  /** A connection target: "host:port" or an object. */
  export type Address = string | { hostname?: string; host?: string; port: number };

  /** Metadata about an established socket. */
  export interface SocketInfo {
    remoteAddress: string;
    remotePort: number;
    localAddress: string;
    localPort: number;
  }

  /** Options for {@link connect} (the WinterTC Sockets API shape). */
  export interface ConnectOptions {
    /** `"on"` negotiates TLS; `"starttls"` is reserved (not yet supported). */
    secureTransport?: "off" | "on" | "starttls";
    allowHalfOpen?: boolean;
  }

  /** A duplex TCP socket. All I/O is via the web streams; nothing blocks. */
  export interface Socket {
    /** Incoming bytes. */
    readonly readable: ReadableStream<Uint8Array>;
    /** Outgoing bytes; closing the writer half-closes (sends FIN). */
    readonly writable: WritableStream<Uint8Array>;
    /** Resolves once connected, with the socket's address info. */
    readonly opened: Promise<SocketInfo>;
    /** Resolves when the socket is fully closed. */
    readonly closed: Promise<void>;
    /** Fully close the socket. */
    close(): Promise<void>;
    /** Upgrade to TLS (not yet supported). */
    startTls(): Socket;
  }

  /** Options for {@link listen}. */
  export interface ListenOptions {
    hostname?: string;
    host?: string;
    /** `0` binds an ephemeral port (read it back from `addr`). */
    port: number;
  }

  /** A listening socket — an async-iterable of incoming {@link Socket}s. */
  export interface Listener extends AsyncIterable<Socket> {
    /** The bound address (resolves after the bind completes). */
    readonly addr: Promise<{ hostname: string; port: number }>;
    /** Accept the next connection (`null` once closed). */
    accept(): Promise<Socket | null>;
    /** Stop listening. */
    close(): Promise<void>;
  }

  /** Open an outbound TCP connection (capability: `Net`). Returns immediately. */
  export function connect(address: Address, options?: ConnectOptions): Socket;

  /** Bind a listening socket (capability: `NetListen`). */
  export function listen(options: ListenOptions): Listener;

  const net: { connect: typeof connect; listen: typeof listen };
  export default net;
}
