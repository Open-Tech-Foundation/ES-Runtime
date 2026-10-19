/**
 * The byte pipe under the connection, and the seam that lets a test stand in
 * for it.
 *
 * Everything above this is NATS — commands in, commands out — and everything
 * below is a socket. The connection never touches `runtime:net` itself; it is
 * handed a {@link Transport}, which is why the handshake, the reconnect and the
 * command pump can be tested against a scripted server in memory, with no port
 * and no capability granted (D65: `esrun` grants nothing by default, and a unit
 * test should not be the reason to start).
 *
 * The two methods are the whole contract: pull the next chunk, or `null` for
 * end of stream, and push bytes. A read that rejects is a lost connection, and
 * the connection layer treats it as a loss to recover from — either by
 * reconnecting or by latching the cause, never by resynchronising a stream it
 * can no longer trust.
 */

import { connect as netConnect } from "runtime:net";

import { NatsError, NatsErrorCode } from "./errors.js";

/** A duplex byte pipe. */
export interface Transport {
  /** The next chunk from the peer, or `null` once the peer has finished. */
  read(): Promise<Uint8Array | null>;
  /** Sends bytes, resolving once the stream has accepted them. */
  write(bytes: Uint8Array): Promise<void>;
  /**
   * Upgrades the pipe to TLS in place, for a transport opened in "starttls"
   * mode.
   *
   * NATS does not negotiate TLS at connect: the server opens in the clear, sends
   * INFO, and only then — when it reports `tls_required` — does the client start
   * the handshake. A transport that cannot upgrade (a plaintext one, or a test
   * double) leaves this undefined, and the connection treats a TLS request it
   * cannot honour as a handshake failure rather than writing CONNECT in the
   * clear.
   */
  startTls?(): Promise<void>;
  /**
   * Closes both directions.
   *
   * With `abort`, a write still pending is dropped rather than waited for. The
   * failure path needs that: a write stuck on a peer that has stopped reading
   * makes a graceful close wait on it forever, so the socket close that would
   * interrupt it is never reached and the socket is left open.
   */
  close(options?: { abort?: boolean }): Promise<void>;
}

/** What `runtime:net`'s `connect()` hands back. */
type Socket = ReturnType<typeof netConnect>;

/** Where and how to reach a server. */
export interface NetTransportOptions {
  hostname: string;
  port: number;
  /** Negotiate TLS, in band, once the server's INFO asks for it. */
  tls?: boolean;
  /** Extra trust anchors, as PEM, for a private certificate authority. */
  ca?: string | Uint8Array;
  /** The name the certificate must carry, when it is not `hostname`. */
  servername?: string;
}

export class NetTransport implements Transport {
  #socket: Socket;
  #reader: ReadableStreamDefaultReader<Uint8Array>;
  #writer: WritableStreamDefaultWriter<Uint8Array>;
  readonly #starttls: boolean;
  #ended = false;

  constructor(socket: Socket, starttls = false) {
    this.#socket = socket;
    this.#starttls = starttls;
    this.#reader = socket.readable.getReader();
    this.#writer = socket.writable.getWriter();
  }

  async read(): Promise<Uint8Array | null> {
    if (this.#ended) return null;
    const { value, done } = await this.#reader.read();
    if (done) {
      this.#ended = true;
      return null;
    }
    return value ?? new Uint8Array(0);
  }

  async write(bytes: Uint8Array): Promise<void> {
    await this.#writer.write(bytes);
  }

  async startTls(): Promise<void> {
    if (!this.#starttls) {
      throw new NatsError("this transport cannot be upgraded to TLS", {
        code: NatsErrorCode.Handshake,
      });
    }
    // Release the locks first: the upgrade reads and writes the underlying
    // connection to run the handshake, and it cannot take its own reader while
    // ours is held. Nothing is pending here — the connection upgrades between
    // reading INFO and writing CONNECT.
    await this.#reader.releaseLock();
    await this.#writer.releaseLock();
    try {
      const upgraded = this.#socket.startTls();
      this.#socket = upgraded;
      this.#reader = upgraded.readable.getReader();
      this.#writer = upgraded.writable.getWriter();
      await upgraded.opened;
    } catch (e) {
      const detail = e instanceof Error ? e.message : String(e);
      throw new NatsError(`the TLS handshake failed: ${detail}`, {
        code: NatsErrorCode.Connection,
        cause: e,
      });
    }
  }

  async close(options: { abort?: boolean } = {}): Promise<void> {
    this.#ended = true;
    if (options.abort === true) {
      // The socket, and only the socket. It tears the connection down, which
      // settles a write the peer has stopped reading. Do not wait on the writer:
      // `writer.close()` waits for that write, and `writer.abort()` does too once
      // the write is in flight — each is the hang this path exists to avoid.
      await this.#socket.close().catch(() => {});
      return;
    }
    // The writer first, so the peer sees a tidy half-close when it is still
    // listening; a socket closed under a pending write is an error nobody reads.
    await this.#writer.close().catch(() => {});
    await this.#socket.close().catch(() => {});
  }
}

/**
 * Opens a socket to a server.
 *
 * The capability check happens inside `runtime:net`'s `connect`, so a host that
 * `--allow-net` does not name is refused there (`ERR_CAPABILITY_DENIED`), not
 * here — the host allowlist (D38) is the runtime's, and this package does not
 * get to have a second opinion about it.
 */
export async function netTransport(options: NetTransportOptions): Promise<Transport> {
  let socket: Socket;
  try {
    socket = netConnect(
      { hostname: options.hostname, port: options.port },
      options.tls === true
        ? {
            // "starttls", not "on": NATS reads INFO in the clear and begins the
            // handshake only after the server says TLS is required.
            secureTransport: "starttls",
            ...(options.ca === undefined ? {} : { ca: options.ca }),
            ...(options.servername === undefined ? {} : { sni: options.servername }),
          }
        : {},
    );
    await socket.opened;
  } catch (e) {
    const detail = e instanceof Error ? e.message : String(e);
    throw new NatsError(
      `the connection to ${options.hostname}:${options.port} could not be opened: ${detail}`,
      { code: NatsErrorCode.Connection, cause: e },
    );
  }
  return new NetTransport(socket, options.tls === true);
}
