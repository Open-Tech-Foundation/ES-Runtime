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
     * Extra trust anchors, as PEM certificates (a string or bytes).
     *
     * **Added** to the built-in roots, never instead of them: naming a private
     * certificate authority does not stop the program trusting the public ones,
     * and it can only make verification accept more certificates — the hostname
     * and chain checks still run. A server matching neither is still refused.
     */
    ca?: string | Uint8Array | ArrayBuffer | ArrayBufferView;
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
    /** Fully close the socket. `reason` is advisory (WinterTC) and ignored. */
    close(reason?: unknown): Promise<void>;
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

  /** One received datagram. The sender travels with the message, because on an
   * unconnected socket it differs for every one. */
  export interface Datagram {
    /** The payload, exactly as one datagram carried it. */
    data: Uint8Array;
    /** The sender's address (an IP literal). */
    address: string;
    /** The sender's port. */
    port: number;
  }

  /** Options for {@link bind}. */
  export interface BindOptions {
    hostname?: string;
    host?: string;
    /** `0` binds an ephemeral port (read it back from `addr`). */
    port: number;
    /**
     * Bind with `SO_REUSEPORT`, so several **processes** can share this address
     * and the kernel distributes datagrams across them. **Unix only.**
     *
     * @defaultValue `false`
     */
    reusePort?: boolean;
    /**
     * Bind with `SO_REUSEADDR`, so another socket may hold this address too —
     * what lets two processes on one machine both receive a multicast group
     * (mDNS, SSDP).
     *
     * @defaultValue `false`
     */
    reuseAddress?: boolean;
    /**
     * Permit sending to the broadcast address (`SO_BROADCAST`). IPv4 only —
     * IPv6 has no broadcast, and asking for it on a v6 socket is an error.
     *
     * @defaultValue `false`
     */
    broadcast?: boolean;
    /** Hop limit for unicast datagrams (0–255). Omitted ⇒ the OS default. */
    ttl?: number;
    /**
     * Hop limit for multicast datagrams (0–255). Omitted ⇒ the OS default,
     * which is `1`: a datagram that does not leave the local segment.
     */
    multicastTtl?: number;
    /**
     * Whether multicast sends are also delivered back to this host. Omitted ⇒
     * the OS default (on). Turn it off so a sender does not receive its own
     * announcements.
     */
    multicastLoopback?: boolean;
  }

  /** Options for {@link DatagramSocket.joinMulticast}. */
  export interface MulticastOptions {
    /**
     * Which local interface carries the membership: an IPv4 address for a v4
     * group, an interface **index** for a v6 one. Omitted ⇒ the OS chooses,
     * which is only unambiguous on a host with one interface.
     */
    interface?: string | number;
  }

  /**
   * A bound UDP socket — messages, not a byte stream, so it has `send`/`receive`
   * rather than `readable`/`writable`.
   */
  export interface DatagramSocket extends AsyncIterable<Datagram> {
    /** The bound address (resolves after the bind completes). */
    readonly addr: Promise<{ hostname: string; port: number }>;
    /** Resolves when the socket is closed. */
    readonly closed: Promise<void>;
    /**
     * Send one datagram, resolving with the number of bytes sent. `address` is
     * required unless the socket is {@link connect}ed.
     */
    send(
      data: string | Uint8Array | ArrayBuffer | ArrayBufferView,
      address?: Address,
    ): Promise<number>;
    /**
     * The next datagram, or `null` once closed. One call is one message —
     * including a zero-length one, which is a message and not an end of stream.
     */
    receive(): Promise<Datagram | null>;
    /**
     * Fix the peer: later sends need no address, and datagrams from anyone else
     * are discarded. No packet is sent (UDP has no handshake), so this succeeds
     * against a host that is not listening.
     */
    connect(address: Address): Promise<Omit<SocketInfo, "alpn">>;
    /** Join a multicast group. */
    joinMulticast(group: string, options?: MulticastOptions): Promise<void>;
    /** Leave a multicast group. */
    leaveMulticast(group: string, options?: MulticastOptions): Promise<void>;
    /** Close the socket. A parked {@link receive} resolves to `null`. */
    close(): Promise<void>;
  }

  /** Open an outbound TCP connection (capability: `Net`). Returns immediately. */
  export function connect(address: Address, options?: ConnectOptions): Socket;

  /** Bind a listening socket (capability: `NetListen`). */
  export function listen(options: ListenOptions): Listener;

  /**
   * Bind a UDP socket (capability: `NetListen` — this takes a port, and a port
   * is how a process is reached). Sending needs `Net` as well, checked per
   * datagram.
   */
  export function bind(options: BindOptions): DatagramSocket;

  const net: { connect: typeof connect; listen: typeof listen; bind: typeof bind };
  export default net;
}
