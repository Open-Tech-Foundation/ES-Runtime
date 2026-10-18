/**
 * The byte pipe under the connection, and the seam that lets a test stand in
 * for it.
 *
 * Everything above this is MQTT — packets in, packets out — and everything
 * below is a socket. The connection never touches `runtime:net` itself; it is
 * handed a {@link Transport}, which is why the handshake and the packet pump
 * can be tested against a scripted broker in memory, with no port and no
 * capability granted (D65: `esrun` grants nothing by default, and a unit test
 * should not be the reason to start).
 *
 * The two methods are the whole contract: pull the next chunk, or `null` for
 * end of stream, and push bytes. A read that rejects is a lost connection, and
 * the connection layer latches that rather than retrying — a half-read packet
 * is not something a stream can resynchronise from.
 */

import { connect as netConnect } from "runtime:net";

import { MqttError, MqttErrorCode } from "./errors.js";

/** A duplex byte pipe. */
export interface Transport {
  /** The next chunk from the peer, or `null` once the peer has finished. */
  read(): Promise<Uint8Array | null>;
  /** Sends bytes, resolving once the stream has accepted them. */
  write(bytes: Uint8Array): Promise<void>;
  /** Closes both directions. */
  close(): Promise<void>;
}

/** What `runtime:net`'s `connect()` hands back. */
type Socket = ReturnType<typeof netConnect>;

/** Where and how to reach a broker. */
export interface NetTransportOptions {
  hostname: string;
  port: number;
  /** Negotiate TLS on connect — MQTT itself has no in-band upgrade. */
  tls?: boolean;
  /** Extra trust anchors, as PEM, for a private certificate authority. */
  ca?: string | Uint8Array;
  /** The name the certificate must carry, when it is not `hostname`. */
  servername?: string;
}

class NetTransport implements Transport {
  readonly #socket: Socket;
  readonly #reader: ReadableStreamDefaultReader<Uint8Array>;
  readonly #writer: WritableStreamDefaultWriter<Uint8Array>;
  #ended = false;

  constructor(socket: Socket) {
    this.#socket = socket;
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
    // A zero-length chunk is not end of stream, but it carries nothing either,
    // so it is asked for again rather than handed on as an empty packet's start.
    return value ?? new Uint8Array(0);
  }

  async write(bytes: Uint8Array): Promise<void> {
    await this.#writer.write(bytes);
  }

  async close(): Promise<void> {
    this.#ended = true;
    // The writer first, so the peer sees a tidy half-close when it is still
    // listening; a socket closed under a pending write is an error nobody reads.
    await this.#writer.close().catch(() => {});
    await this.#socket.close().catch(() => {});
  }
}

/**
 * Opens a socket to a broker.
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
            secureTransport: "on",
            ...(options.ca === undefined ? {} : { ca: options.ca }),
            ...(options.servername === undefined ? {} : { sni: options.servername }),
          }
        : {},
    );
    await socket.opened;
  } catch (e) {
    const detail = e instanceof Error ? e.message : String(e);
    throw new MqttError(
      `the connection to ${options.hostname}:${options.port} could not be opened: ${detail}`,
      { code: MqttErrorCode.Connection, cause: e },
    );
  }
  return new NetTransport(socket);
}
