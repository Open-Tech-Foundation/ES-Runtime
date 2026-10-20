/**
 * The pool: a few connections, lent out one at a time, dialled again when one
 * dies.
 *
 * One AMQP connection multiplexes many channels, so the pool exists for two
 * reasons only. The first is concurrency: two publishers on one connection share
 * one socket, and the connection layer already serialises their frames, so a
 * burst can be spread over several connections instead of queuing behind itself.
 * The second is recovery.
 *
 * A connection is a socket, and once its stream is broken a later frame cannot
 * be trusted — the connection layer latches that and hands every caller the one
 * cause. So the pool's whole recovery story is to notice the latched failure and
 * dial again, and nothing more. It does **not** replay topology or re-subscribe:
 * the broker cannot restore an exclusive or auto-delete resource, so a silent
 * replay would hide the gap rather than close it. Re-establishing what an
 * application declared is the application's, from {@link PoolOptions.onConnect}.
 *
 * Where the URL and the settings disagree, the settings win: the URL says what
 * to reach and the credentials to reach it with, and a setting overrides any of
 * them.
 */

import type { Channel } from "./channel.js";
import { Connection, type ConnectionSettings } from "./connection.js";
import { RabbitError, RabbitErrorCode } from "./errors.js";
import { type NetTransportOptions, netTransport, type Transport } from "./transport.js";

/** The pieces of an `amqp://` or `amqps://` URL. */
export interface BrokerAddress {
  hostname: string;
  port: number;
  /** Whether the scheme was `amqps:` — TLS on connect. */
  tls: boolean;
  username: string;
  password: string;
  vhost: string;
  /** `?heartbeat=` — seconds to ask for. */
  heartbeat?: number;
  /** `?channel_max=` — channels to ask for. */
  channelMax?: number;
  /** `?frame_max=` — the largest frame to accept, in bytes. */
  frameMax?: number;
  /** `?connection_timeout=` — how long the socket may take to open, in ms. */
  connectTimeout?: number;
  /** `?server_name_indication=` — the name the certificate must carry. */
  servername?: string;
}

/**
 * What {@link openConnection} and a {@link Pool} take: the connection settings,
 * plus how the socket is opened.
 */
export interface OpenSettings extends ConnectionSettings {
  /**
   * How long the socket may take to open, in ms; `0` waits as long as the
   * operating system does. Defaults to `timeout` (30 000).
   */
  connectTimeout?: number;
  /** Extra trust anchors for `amqps:`, as PEM — a private certificate authority. */
  ca?: string | Uint8Array;
  /** The name the broker's certificate must carry, when it is not the URL's host. */
  servername?: string;
}

/** The connections the pool holds when nothing says otherwise. */
const DEFAULT_POOL_SIZE = 4;

/** RabbitMQ's own defaults, for a URL that names none of them. */
const DEFAULT_PORT = 5672;
const DEFAULT_TLS_PORT = 5671;
const DEFAULT_USERNAME = "guest";
const DEFAULT_PASSWORD = "guest";
const DEFAULT_VHOST = "/";
/** How long the socket may take to open when nothing else says, in ms. */
const DEFAULT_TIMEOUT = 30_000;

/**
 * Reads a broker URL.
 *
 * The username, password, and virtual host default to RabbitMQ's own: `guest`,
 * `guest`, and `/`. The port defaults to the scheme's own, `5672` for `amqp:`
 * and `5671` for `amqps:`, so a `amqps://` URL without a port reaches the TLS
 * listener rather than the plain one. The path is one segment and is the vhost,
 * so a vhost that contains a slash is written percent-encoded (`%2F`).
 */
export function parseAmqpUrl(url: string): BrokerAddress {
  const parsed = new URL(url);
  if (parsed.protocol !== "amqp:" && parsed.protocol !== "amqps:") {
    throw new RabbitError(`"${parsed.protocol}" is not an amqp: or amqps: URL`, {
      code: RabbitErrorCode.Connection,
    });
  }
  const tls = parsed.protocol === "amqps:";
  const address: BrokerAddress = {
    // `URL` keeps an IPv6 literal's brackets; the socket wants the address.
    hostname: parsed.hostname.replace(/^\[(.*)\]$/, "$1"),
    port: parsed.port === "" ? (tls ? DEFAULT_TLS_PORT : DEFAULT_PORT) : Number(parsed.port),
    tls,
    username: parsed.username === "" ? DEFAULT_USERNAME : decodeURIComponent(parsed.username),
    password: parsed.password === "" ? DEFAULT_PASSWORD : decodeURIComponent(parsed.password),
    vhost:
      parsed.pathname.length > 1 ? decodeURIComponent(parsed.pathname.slice(1)) : DEFAULT_VHOST,
  };
  readQuery(parsed.searchParams, address);
  return address;
}

/**
 * The query parameters RabbitMQ's own clients read (rabbitmq.com/docs/
 * uri-query-parameters), plus `frame_max`, which the other clients accept.
 *
 * One that cannot be honoured is an error rather than ignored. The TLS file
 * parameters name paths on disk, which a URL must not be able to make this
 * process read (the `ca` setting takes the PEM itself); `verify=verify_none`
 * asks to skip certificate checks, which `runtime:net` does not do; and a typo
 * silently dropped is a setting the caller believes is in force.
 */
function readQuery(query: URLSearchParams, address: BrokerAddress): void {
  const mechanisms: string[] = [];
  for (const [name, value] of query) {
    switch (name) {
      case "heartbeat":
        address.heartbeat = integer(name, value, 0xffff);
        break;
      case "channel_max":
        address.channelMax = integer(name, value, 0xffff);
        break;
      case "frame_max":
        address.frameMax = integer(name, value, 0xffffffff);
        break;
      case "connection_timeout":
        address.connectTimeout = integer(name, value, Number.MAX_SAFE_INTEGER);
        break;
      case "auth_mechanism":
        mechanisms.push(value.toLowerCase());
        break;
      case "server_name_indication":
        if (!address.tls) throw unsupported(`${name} applies only to an amqps: URL`);
        address.servername = value;
        break;
      case "verify":
        if (value !== "verify_peer") {
          throw unsupported(`verify=${value}: the broker's certificate is always verified`);
        }
        break;
      case "cacertfile":
        throw unsupported(`${name}: pass the certificate's PEM as the \`ca\` setting instead`);
      case "certfile":
      case "keyfile":
      case "password":
        throw unsupported(`${name}: client certificates are not supported`);
      default:
        throw unsupported(`the URL query parameter ${name} is not one this client knows`);
    }
  }
  if (mechanisms.length > 0 && !mechanisms.includes("plain")) {
    throw unsupported(`auth_mechanism=${mechanisms.join(",")}: only PLAIN is supported`);
  }
}

function integer(name: string, value: string, max: number): number {
  const n = /^\d+$/.test(value) ? Number(value) : Number.NaN;
  if (!Number.isSafeInteger(n) || n > max) {
    throw new RabbitError(`the URL query parameter ${name} must be an integer in 0..${max}`, {
      code: RabbitErrorCode.Connection,
    });
  }
  return n;
}

function unsupported(message: string): RabbitError {
  return new RabbitError(message, { code: RabbitErrorCode.Unsupported });
}

/**
 * Opens one connection to `url` and completes the handshake.
 *
 * `dial` is the seam between the address and the socket: it defaults to
 * {@link netTransport}, and a test passes a scripted transport so a connection
 * is opened with no port and no capability granted.
 */
export async function openConnection(
  url: string,
  settings: OpenSettings = {},
  dial: (options: NetTransportOptions) => Promise<Transport> = netTransport,
): Promise<Connection> {
  const address = parseAmqpUrl(url);
  const servername = settings.servername ?? address.servername;
  const transport = await dial({
    hostname: address.hostname,
    port: address.port,
    tls: address.tls,
    connectTimeout:
      settings.connectTimeout ?? address.connectTimeout ?? settings.timeout ?? DEFAULT_TIMEOUT,
    ...(settings.ca === undefined ? {} : { ca: settings.ca }),
    ...(servername === undefined ? {} : { servername }),
  });
  const heartbeat = settings.heartbeat ?? address.heartbeat;
  const channelMax = settings.channelMax ?? address.channelMax;
  const frameMax = settings.frameMax ?? address.frameMax;
  const connection = new Connection(transport, {
    username: settings.username ?? address.username,
    password: settings.password ?? address.password,
    vhost: settings.vhost ?? address.vhost,
    ...(heartbeat === undefined ? {} : { heartbeat }),
    ...(channelMax === undefined ? {} : { channelMax }),
    ...(frameMax === undefined ? {} : { frameMax }),
    ...(settings.locale === undefined ? {} : { locale: settings.locale }),
    ...(settings.clientProperties === undefined
      ? {}
      : { clientProperties: settings.clientProperties }),
    ...(settings.handshakeTimeout === undefined
      ? {}
      : { handshakeTimeout: settings.handshakeTimeout }),
    ...(settings.timeout === undefined ? {} : { timeout: settings.timeout }),
  });
  await connection.open();
  return connection;
}

/** Connection settings, plus how big the pool is and how it dials. */
export interface PoolOptions extends OpenSettings {
  /** The most connections held open at once. Default 4. */
  size?: number;
  /**
   * Called with each connection once it is open and before it is lent out — the
   * first one and every replacement. This is where an application re-declares
   * its topology and re-subscribes after a connection was lost. A failure here
   * discards the connection and fails the borrow that caused it.
   */
  onConnect?: (connection: Connection) => void | Promise<void>;
  /**
   * How to open the socket. Defaults to {@link netTransport}; a test substitutes
   * a scripted transport so the pool runs with no port open.
   */
  dial?: (options: NetTransportOptions) => Promise<Transport>;
}

/** A borrow waiting for a connection to be free or for one to be dialled. */
interface Waiter {
  resolve: (connection: Connection) => void;
  reject: (error: RabbitError) => void;
}

/**
 * A pool of connections to one broker.
 *
 * A connection is borrowed whole — `withConnection` for as long as a function
 * runs, or `acquire` and `release` for a longer lease such as a subscription's —
 * and a borrowed connection is never shared. `withChannel` opens and closes a
 * channel around the function, which is all most work needs.
 */
export class Pool {
  readonly #url: string;
  readonly #options: PoolOptions;
  readonly #size: number;
  readonly #onConnect: ((connection: Connection) => void | Promise<void>) | null;
  readonly #dial: (options: NetTransportOptions) => Promise<Transport>;

  #all = new Set<Connection>();
  #idle: Connection[] = [];
  #busy = new Set<Connection>();
  #waiters: Waiter[] = [];
  // Borrows that are dialling right now. Counted apart from `#all`, because the
  // slot they will fill is spoken for before the socket exists.
  #opening = 0;
  #closed = false;

  constructor(url: string, options: PoolOptions = {}) {
    this.#url = url;
    this.#options = options;
    this.#size = options.size ?? DEFAULT_POOL_SIZE;
    this.#onConnect = options.onConnect ?? null;
    this.#dial = options.dial ?? netTransport;
  }

  /** The connections open right now, idle and busy together. */
  get size(): number {
    return this.#all.size;
  }

  /** The connections idle, ready to be lent. */
  get idle(): number {
    return this.#idle.length;
  }

  /** The connections lent out. */
  get busy(): number {
    return this.#busy.size;
  }

  /**
   * Borrows a connection.
   *
   * An idle live one is handed over; if there is room a fresh one is dialled;
   * otherwise the borrow waits for a release. A connection that came back dead
   * was dropped, so what is handed over is always one the pool believes is
   * usable.
   */
  async acquire(): Promise<Connection> {
    this.#requireOpen();
    const idle = this.#take();
    if (idle !== null) {
      this.#busy.add(idle);
      return idle;
    }
    if (this.#all.size + this.#opening < this.#size) return this.#spawn();
    return new Promise<Connection>((resolve, reject) => {
      this.#waiters.push({ resolve, reject });
    });
  }

  /**
   * Returns a borrowed connection.
   *
   * A connection that is still usable goes back to the idle pile; one that has
   * died is closed and forgotten, which is what frees its slot for the next
   * borrow to dial a replacement. A connection the pool did not lend is ignored.
   */
  release(connection: Connection): void {
    if (!this.#busy.delete(connection)) return;
    if (connection.usable) this.#idle.push(connection);
    else this.#discard(connection);
    this.#serve();
  }

  /** Borrows a connection for the length of `fn`, then returns it. */
  async withConnection<T>(fn: (connection: Connection) => Promise<T>): Promise<T> {
    const connection = await this.acquire();
    try {
      return await fn(connection);
    } finally {
      this.release(connection);
    }
  }

  /**
   * Borrows a connection, opens a channel on it, runs `fn`, and closes the
   * channel. The channel is closed whether `fn` returned or threw; a channel on
   * a connection that died in the meantime is already gone, and says so.
   */
  async withChannel<T>(fn: (channel: Channel) => Promise<T>): Promise<T> {
    return this.withConnection(async (connection) => {
      const channel = await connection.channel();
      try {
        return await fn(channel);
      } finally {
        await channel.close().catch(() => {});
      }
    });
  }

  /**
   * Closes every connection and refuses further borrows.
   *
   * A borrow that is only waiting is failed with `ERR_RABBITMQ_CLOSED`; one that
   * is already dialling has its connection closed the moment it is open. Nothing
   * waits on a broker that has stopped answering.
   */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    const error = new RabbitError("the pool is closed", { code: RabbitErrorCode.Closed });
    for (const waiter of this.#waiters.splice(0)) waiter.reject(error);
    const connections = [...this.#all];
    this.#all.clear();
    this.#idle.length = 0;
    this.#busy.clear();
    await Promise.all(connections.map((connection) => connection.close().catch(() => {})));
  }

  // -- the machinery --------------------------------------------------------

  #requireOpen(): void {
    if (this.#closed) {
      throw new RabbitError("the pool is closed", { code: RabbitErrorCode.Closed });
    }
  }

  /** Pops a live idle connection, dropping any dead ones it passes. */
  #take(): Connection | null {
    while (this.#idle.length > 0) {
      const connection = this.#idle.pop()!;
      if (connection.usable) return connection;
      this.#discard(connection);
    }
    return null;
  }

  /**
   * Dials a connection and reserves its slot.
   *
   * `#opening` is incremented before the first await, so two borrows issued in
   * the same turn cannot both decide there is room for a new one.
   */
  async #spawn(): Promise<Connection> {
    this.#opening++;
    let connection: Connection;
    try {
      connection = await this.#connect();
    } catch (e) {
      this.#opening--;
      // The slot this dial held is free again. A borrow that queued because
      // the pool looked full is waiting for exactly that, and with no release
      // coming (a pool of one, say) nothing else would ever wake it.
      this.#serve();
      throw e;
    }
    this.#opening--;
    this.#busy.add(connection);
    return connection;
  }

  /** Dials, registers, and announces one connection. */
  async #connect(): Promise<Connection> {
    const connection = await openConnection(this.#url, this.#options, this.#dial);
    // A close may have overtaken the dial: this connection has no owner now, so
    // it is closed rather than leaked.
    if (this.#closed) {
      await connection.close().catch(() => {});
      throw new RabbitError("the pool is closed", { code: RabbitErrorCode.Closed });
    }
    this.#all.add(connection);
    if (this.#onConnect !== null) {
      try {
        await this.#onConnect(connection);
      } catch (e) {
        this.#all.delete(connection);
        await connection.close().catch(() => {});
        throw e;
      }
    }
    // `close()` may have run while an asynchronous onConnect was declaring
    // topology. Do not hand the now-closed connection to the borrow that was
    // waiting for setup to finish.
    if (this.#closed) {
      this.#all.delete(connection);
      await connection.close().catch(() => {});
      throw new RabbitError("the pool is closed", { code: RabbitErrorCode.Closed });
    }
    return connection;
  }

  /** Forgets a connection and closes it, swallowing a close that cannot happen. */
  #discard(connection: Connection): void {
    this.#all.delete(connection);
    this.#busy.delete(connection);
    void connection.close().catch(() => {});
  }

  /** Hands released connections to borrows that were waiting. */
  #serve(): void {
    while (this.#waiters.length > 0) {
      const idle = this.#take();
      if (idle !== null) {
        const waiter = this.#waiters.shift()!;
        this.#busy.add(idle);
        waiter.resolve(idle);
        continue;
      }
      if (this.#all.size + this.#opening >= this.#size) return;
      const waiter = this.#waiters.shift()!;
      this.#opening++;
      void this.#connect().then(
        (connection) => {
          this.#opening--;
          this.#busy.add(connection);
          waiter.resolve(connection);
        },
        (error) => {
          this.#opening--;
          waiter.reject(error);
          this.#serve();
        },
      );
    }
  }
}
