declare module "runtime:net" {
  /** A connection target: "host:port" or an object. */
  export type Address = string | { hostname?: string; host?: string; port: number };

  /** Metadata about an established socket. */
  export interface SocketInfo {
    /** Remote peer as WinterTC `"host:port"` (IPv6 host bracketed). */
    remoteAddress: string;
    remotePort: number;
    /** Local end as WinterTC `"host:port"` (IPv6 host bracketed). */
    localAddress: string;
    localPort: number;
    /** Negotiated ALPN protocol (TLS only; `null` for plaintext or none). */
    alpn: string | null;
  }

  /** Options for {@link connect} (the WinterTC Sockets API shape). */
  export interface ConnectOptions {
    /**
     * `"on"` negotiates TLS immediately; `"starttls"` opens plaintext and may be
     * upgraded later via {@link Socket.startTls}; `"off"` (default) is plain TCP.
     */
    secureTransport?: "off" | "on" | "starttls";
    /** TLS server name (SNI + cert verification); defaults to the connect host. */
    sni?: string;
    /** ALPN protocols to offer, in preference order. */
    alpn?: string[];
    /**
     * Keep the writable usable after the peer's FIN (read EOF) instead of tearing
     * the whole socket down. Defaults to `false` (WinterTC).
     */
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
    /** `true` once this socket is the result of a {@link startTls} upgrade. */
    readonly upgraded: boolean;
    /**
     * Upgrade a `secureTransport: "starttls"` socket to TLS in place, returning
     * a new {@link Socket} for the encrypted stream (`upgraded === true`). The
     * original socket is consumed. Throws on a non-`"starttls"` socket.
     */
    startTls(): Socket;
  }

  /** Options for {@link listen}. */
  export interface ListenOptions {
    hostname?: string;
    host?: string;
    /** `0` binds an ephemeral port (read it back from `addr`). */
    port: number;
    /**
     * `"on"` terminates TLS: every accepted {@link Socket} is encrypted and its
     * `opened.alpn` reports the negotiated protocol. Requires {@link cert} and
     * {@link key}. Defaults to `"off"` (plain TCP).
     */
    secureTransport?: "off" | "on";
    /** PEM certificate chain (leaf first), as a string or bytes. Required for TLS. */
    cert?: string | Uint8Array;
    /** PEM private key (PKCS#8/PKCS#1/SEC1), as a string or bytes. Required for TLS. */
    key?: string | Uint8Array;
    /** ALPN protocols to advertise, in preference order. */
    alpn?: string[];
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
