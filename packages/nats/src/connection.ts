/**
 * The connection: one socket, one NATS session, and the framing, keep-alive and
 * reconnection that keep it honest.
 *
 * Three things this layer owns and no layer above it should have to:
 *
 * - **The command boundary.** NATS is a line protocol with a byte-counted body,
 *   and a socket delivers it in whatever chunks it likes. This layer accumulates
 *   bytes and is what decides a command is complete — an `INFO` split across two
 *   reads, a `MSG` whose payload has not arrived, several commands in one chunk
 *   are all its problem, solved once.
 * - **The protocol's own chatter.** A server `PING` is answered with `PONG`; a
 *   `PONG` settles the oldest `PING` this client sent, in order; a `-ERR` is the
 *   server's only refusal and it is surfaced, usually fatally.
 * - **Recovery.** A lost socket is not fatal here. The connection dials the next
 *   server in its pool, re-runs the handshake, re-establishes every subscription
 *   under its original id, and lets callers that wrote while it was down proceed —
 *   all without the layer above seeing anything but a `Closed` connection or a
 *   recovered one.
 *
 * What it deliberately does not own: routing messages to callers (that is the
 * client's, keyed on the subscription id the server names), and the decision to
 * give up — that is the reconnect policy's, expressed in settings.
 */

import { unrefTimer } from "runtime:process";
import { NatsError, NatsErrorCode, ProtocolError } from "./errors.js";
import { decodeHeaders, encodeHeaders, type HeaderStatus } from "./protocol/headers.js";
import { validateQueue, validateSubject } from "./protocol/subject.js";
import {
  CLIENT_LANG,
  type ConnectOptions,
  DEFAULT_MAX_PAYLOAD,
  drain,
  encodeConnect,
  encodeHpub,
  encodePing,
  encodePong,
  encodePub,
  encodeSub,
  encodeUnsub,
  NATS_PROTOCOL,
  type ServerCommand,
  type ServerInfo,
} from "./protocol/wire.js";
import { type NetTransportOptions, netTransport, type Transport } from "./transport.js";

/** NATS's default client port, plain and TLS alike. */
export const DEFAULT_NATS_PORT: number = 4222;
/** How often to ping an idle connection, in ms. */
export const DEFAULT_PING_INTERVAL: number = 120_000;
/** How long the handshake, and any one operation, may take, in ms. */
export const DEFAULT_TIMEOUT: number = 30_000;
/** How many times to redial before giving up, by default. */
export const DEFAULT_MAX_RECONNECT_ATTEMPTS: number = 10;
/** How long to wait between redials, in ms. */
export const DEFAULT_RECONNECT_WAIT: number = 2_000;

/** A reached server, parsed out of a URL or an `INFO`. */
export interface NatsAddress {
  hostname: string;
  port: number;
  tls: boolean;
}

/** How to redial after a lost connection. */
export interface ReconnectSettings {
  enabled: boolean;
  maxAttempts: number;
  wait: number;
}

/** How to identify the client, what session to ask for, and how to recover it. */
export interface ConnectionSettings {
  /** Extra servers to try, beyond the one the URL named. */
  servers?: Array<NatsAddress | string>;
  username?: string;
  password?: string;
  /** A token credential, sent as `auth_token`. */
  token?: string;
  /** A connection name, shown in the server's monitoring. */
  name?: string;
  /** Ask the server to acknowledge every command with `+OK`. Defaults `false`. */
  verbose?: boolean;
  /** Ask the server to validate subjects strictly. Defaults `false`. */
  pedantic?: boolean;
  /** Receive this client's own publishes on its subscriptions. Defaults `true`. */
  echo?: boolean;
  /** Advertise header support when the server offers it. Defaults `true`. */
  headers?: boolean;
  /** Advertise no-responder statuses when the server offers headers. Defaults `true`. */
  noResponders?: boolean;
  /** Require TLS on connect. */
  tls?: boolean;
  /** Trust anchors for TLS, as PEM. */
  ca?: string | Uint8Array;
  /** The name the TLS certificate must carry, when it is not the host. */
  servername?: string;
  /** How often to ping an idle connection, in ms. Defaults {@link DEFAULT_PING_INTERVAL}. */
  pingInterval?: number;
  /** How long the handshake may take, in ms. Defaults {@link DEFAULT_TIMEOUT}. */
  timeout?: number;
  /** How to redial after a loss. `false` disables it. Defaults on. */
  reconnect?: boolean | { maxAttempts?: number; wait?: number };
}

/**
 * What the connection tells the client.
 *
 * The connection answers the server's `PING` and settles its own `PONG`s, so what
 * arrives here is what needs an application decision: a message, the loss of the
 * connection, its recovery, and the one news that it is over.
 */
export interface ConnectionHandlers {
  onMessage(message: IncomingMessage): void;
  onDisconnect(error: NatsError): void;
  onReconnect(): void;
  onClosed(error: NatsError | null): void;
}

/** A message as it arrived, with its header block decoded. */
export interface IncomingMessage {
  subject: string;
  sid: number;
  reply: string | null;
  payload: Uint8Array;
  headers: Record<string, string[]>;
  /** A `NATS/1.0` status, when the server attached one (a no-responder, say). */
  status: HeaderStatus | null;
}

function decodeComponent(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    throw new NatsError(`a URL component was not valid percent-encoding: ${value}`, {
      code: NatsErrorCode.Protocol,
    });
  }
}

/**
 * Parses a `nats://` or `tls://` URL into a server and credentials.
 *
 * `nats` is plain on 4222 unless the server demands otherwise; `tls` is TLS from
 * the first byte on 4222. A userinfo with a colon is a user and password; without
 * one it is a token credential, which is how NATS spells them apart.
 */
export function parseNatsUrl(
  url: string,
): NatsAddress & { username?: string; password?: string; token?: string } {
  const match = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/i.exec(url);
  if (match === null) {
    throw new NatsError(`not a NATS URL: ${JSON.stringify(url)}`, { code: NatsErrorCode.Protocol });
  }
  const scheme = match[1]!.toLowerCase();
  if (scheme !== "nats" && scheme !== "tls") {
    throw new NatsError(`unknown NATS scheme ${JSON.stringify(scheme)}`, {
      code: NatsErrorCode.Protocol,
    });
  }
  const tls = scheme === "tls";

  let rest = match[2]!;
  const slash = rest.indexOf("/");
  if (slash >= 0) rest = rest.slice(0, slash);

  let username: string | undefined;
  let password: string | undefined;
  let token: string | undefined;
  const at = rest.lastIndexOf("@");
  if (at >= 0) {
    const userinfo = rest.slice(0, at);
    rest = rest.slice(at + 1);
    const colon = userinfo.indexOf(":");
    if (colon < 0) token = decodeComponent(userinfo);
    else {
      username = decodeComponent(userinfo.slice(0, colon));
      password = decodeComponent(userinfo.slice(colon + 1));
    }
  }

  let hostname: string;
  let port: number;
  if (rest.startsWith("[")) {
    const close = rest.indexOf("]");
    if (close < 0) {
      throw new NatsError(`an IPv6 host is missing its \`]\`: ${url}`, {
        code: NatsErrorCode.Protocol,
      });
    }
    hostname = rest.slice(1, close);
    const after = rest.slice(close + 1);
    port = after.startsWith(":") ? Number(after.slice(1)) : DEFAULT_NATS_PORT;
  } else {
    const colon = rest.lastIndexOf(":");
    if (colon >= 0) {
      hostname = rest.slice(0, colon);
      port = Number(rest.slice(colon + 1));
    } else {
      hostname = rest;
      port = DEFAULT_NATS_PORT;
    }
  }
  if (hostname.length === 0) {
    throw new NatsError(`the URL has no host: ${url}`, { code: NatsErrorCode.Protocol });
  }
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new NatsError(`the URL has an invalid port: ${url}`, { code: NatsErrorCode.Protocol });
  }
  return {
    hostname,
    port,
    tls,
    ...(username === undefined ? {} : { username }),
    ...(password === undefined ? {} : { password }),
    ...(token === undefined ? {} : { token }),
  };
}

/** Parses a `host:port` server entry, or passes a {@link NatsAddress} through. */
export function parseNatsServer(entry: NatsAddress | string, tls = false): NatsAddress {
  if (typeof entry !== "string") return entry;
  if (entry.includes("://")) {
    const parsed = parseNatsUrl(entry);
    return { hostname: parsed.hostname, port: parsed.port, tls: parsed.tls };
  }
  const colon = entry.lastIndexOf(":");
  const hostname = colon < 0 ? entry : entry.slice(0, colon);
  const port = colon < 0 ? DEFAULT_NATS_PORT : Number(entry.slice(colon + 1));
  if (hostname.length === 0 || !Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new NatsError(`not a server address: ${JSON.stringify(entry)}`, {
      code: NatsErrorCode.Protocol,
    });
  }
  return { hostname, port, tls };
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function asNatsError(e: unknown, code: NatsErrorCode = NatsErrorCode.Connection): NatsError {
  return e instanceof NatsError
    ? e
    : new NatsError(e instanceof Error ? e.message : String(e), { code, cause: e });
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    unrefTimer(timer as unknown as number);
  });
}

/** One outstanding `PING`; a `PONG` settles the oldest. */
interface PingWaiter {
  resolve: (() => void) | null;
  reject: ((error: NatsError) => void) | null;
  timer: ReturnType<typeof setTimeout> | null;
}

/** A subscription kept for resubscription after a reconnect. */
interface Registered {
  subject: string;
  queue?: string;
}

export class Connection {
  readonly #settings: ConnectionSettings;
  readonly #handlers: ConnectionHandlers;
  readonly #dial: Dialer;
  #servers: NatsAddress[];

  #transport: Transport | null = null;
  #currentTls = false;
  #info: ServerInfo | null = null;
  #buffer: Uint8Array = new Uint8Array(0);
  #pending: ServerCommand[] = [];

  #connected = false;
  #closing = false;
  #closed = false;
  #failure: NatsError | null = null;
  #reconnecting = false;
  #pump: Promise<void> | null = null;
  #keepAliveTimer: ReturnType<typeof setTimeout> | null = null;
  #lastRead = 0;
  #writeChain: Promise<void> = Promise.resolve();
  #pendingWrites = 0;
  #pings: PingWaiter[] = [];
  #subscriptions = new Map<number, Registered>();
  #nextSid = 1;
  #serverIndex = 0;
  #ready: Promise<void> = Promise.resolve();
  #readyResolve: () => void = () => {};
  #readyReject: (error: NatsError) => void = () => {};

  private constructor(
    settings: ConnectionSettings,
    handlers: ConnectionHandlers,
    dial: Dialer,
    servers: NatsAddress[],
  ) {
    this.#settings = settings;
    this.#handlers = handlers;
    this.#dial = dial;
    this.#servers = servers;
  }

  /** What the server said in its `INFO`. */
  get info(): ServerInfo | null {
    return this.#info;
  }

  /** The server's version string, when it advertised one. */
  get serverVersion(): string | null {
    return this.#info?.version ?? null;
  }

  /** The largest body the server accepts, in bytes. */
  get maxPayload(): number {
    return this.#info?.maxPayload ?? DEFAULT_MAX_PAYLOAD;
  }

  /** Whether the connection is up (and not mid-reconnect). */
  get connected(): boolean {
    return this.#connected;
  }

  /** Whether the connection is over — deliberately closed, or failed. */
  get closed(): boolean {
    return this.#closed;
  }

  /** The failure that ended the connection, or `null` while it stands. */
  get failure(): NatsError | null {
    return this.#failure;
  }

  /** Opens a connection over an already-established transport. */
  static async over(
    transport: Transport,
    settings: ConnectionSettings = {},
    handlers: Partial<ConnectionHandlers> = {},
    dial: Dialer = netTransport,
  ): Promise<Connection> {
    const servers = (settings.servers ?? []).map((entry) =>
      parseNatsServer(entry, settings.tls === true),
    );
    const connection = new Connection(settings, Connection.#handlersOf(handlers), dial, servers);
    await connection.#attach(transport);
    return connection;
  }

  static #handlersOf(handlers: Partial<ConnectionHandlers>): ConnectionHandlers {
    return {
      onMessage: handlers.onMessage ?? (() => {}),
      onDisconnect: handlers.onDisconnect ?? (() => {}),
      onReconnect: handlers.onReconnect ?? (() => {}),
      onClosed: handlers.onClosed ?? (() => {}),
    };
  }

  /** Opens a connection to `url`, or to the first of its servers that answers. */
  static open(
    url: string,
    settings: ConnectionSettings = {},
    handlers: Partial<ConnectionHandlers> = {},
  ): Promise<Connection> {
    return Connection.openWith(url, settings, handlers, netTransport);
  }

  /**
   * {@link Connection.open}, with the dialler made explicit and every server tried.
   *
   * The seam a test stands on: a substituted dialler can hand over a scripted
   * server, and a list can name a second address to fail over to. The handshake
   * is the same one `open` runs.
   */
  static async openWith(
    url: string,
    settings: ConnectionSettings = {},
    handlers: Partial<ConnectionHandlers> = {},
    dial: Dialer = netTransport,
  ): Promise<Connection> {
    const address = parseNatsUrl(url);
    const effective: ConnectionSettings = {
      ...settings,
      ...(settings.username === undefined && address.username !== undefined
        ? { username: address.username }
        : {}),
      ...(settings.password === undefined && address.password !== undefined
        ? { password: address.password }
        : {}),
      ...(settings.token === undefined && address.token !== undefined
        ? { token: address.token }
        : {}),
    };
    const servers = [
      { hostname: address.hostname, port: address.port, tls: address.tls },
      ...(settings.servers ?? []).map((entry) => parseNatsServer(entry, address.tls)),
    ];

    let lastError: NatsError | null = null;
    for (const server of servers) {
      let transport: Transport;
      try {
        transport = await dial({
          hostname: server.hostname,
          port: server.port,
          tls: server.tls,
          ...(settings.ca === undefined ? {} : { ca: settings.ca }),
          ...(settings.servername === undefined ? {} : { servername: settings.servername }),
        });
      } catch (e) {
        lastError = asNatsError(e);
        continue;
      }
      const connection = new Connection(effective, Connection.#handlersOf(handlers), dial, servers);
      try {
        await connection.#attach(transport, server);
        connection.#resolveReady();
        return connection;
      } catch (e) {
        lastError = asNatsError(e, NatsErrorCode.Handshake);
      }
    }
    throw lastError ?? new NatsError("the handshake failed", { code: NatsErrorCode.Handshake });
  }

  async #attach(transport: Transport, address?: NatsAddress): Promise<void> {
    this.#transport = transport;
    this.#currentTls = address?.tls ?? this.#settings.tls === true;
    this.#buffer = new Uint8Array(0);
    this.#pending = [];
    this.#info = null;
    try {
      await this.#handshake();
    } catch (e) {
      await transport.close({ abort: true }).catch(() => {});
      throw e;
    }
    this.#connected = true;
    this.#lastRead = Date.now();
    this.#pump = this.#readLoop();
    this.#armKeepAlive();
  }

  // -- the opening handshake ------------------------------------------------

  async #handshake(): Promise<void> {
    const timeout = this.#settings.timeout ?? DEFAULT_TIMEOUT;
    const first = await this.#nextCommand(timeout);
    if (first === null) {
      throw new NatsError("the server closed the connection during the handshake", {
        code: NatsErrorCode.Connection,
      });
    }
    if (first.kind === "err") {
      throw this.#serverError(first.message, NatsErrorCode.Handshake);
    }
    if (first.kind !== "info") {
      throw new ProtocolError(`the first command from the server was ${first.kind}, not INFO`);
    }
    this.#info = first.info;
    this.#absorbServers(first.info);

    // TLS comes after INFO, never before it: the server opens in the clear and
    // this is where the client learns TLS is wanted. Upgrading now, before
    // CONNECT, is what keeps CONNECT and everything after it off the wire in the
    // clear.
    if (this.#currentTls) {
      const transport = this.#transport;
      if (transport === null || transport.startTls === undefined) {
        throw new NatsError("TLS was requested, but this transport cannot be upgraded to it", {
          code: NatsErrorCode.Handshake,
        });
      }
      await transport.startTls();
    } else if (first.info.tlsRequired) {
      throw new NatsError("the server requires TLS but the connection was opened without it", {
        code: NatsErrorCode.Handshake,
      });
    }

    await this.#writeRaw(encodeConnect(this.#connectOptions()));
    await this.#writeRaw(encodePing());

    for (;;) {
      const command = await this.#nextCommand(timeout);
      if (command === null) {
        throw new NatsError("the server closed the connection during the handshake", {
          code: NatsErrorCode.Connection,
        });
      }
      if (command.kind === "pong") return;
      if (command.kind === "err") {
        throw this.#serverError(command.message, NatsErrorCode.Handshake);
      }
      if (command.kind === "info") {
        this.#info = command.info;
        this.#absorbServers(command.info);
      }
      if (command.kind === "ping") await this.#writeRaw(encodePong());
    }
  }

  #connectOptions(): ConnectOptions {
    const headers = this.#info?.headers === true && this.#settings.headers !== false;
    return {
      verbose: this.#settings.verbose ?? false,
      pedantic: this.#settings.pedantic ?? false,
      tls_required: this.#currentTls,
      ...(this.#settings.name === undefined ? {} : { name: this.#settings.name }),
      lang: CLIENT_LANG,
      protocol: NATS_PROTOCOL,
      echo: this.#settings.echo ?? true,
      headers,
      no_responders: headers && this.#settings.noResponders !== false,
      ...(this.#settings.username === undefined ? {} : { user: this.#settings.username }),
      ...(this.#settings.password === undefined ? {} : { pass: this.#settings.password }),
      ...(this.#settings.token === undefined ? {} : { auth_token: this.#settings.token }),
    };
  }

  #absorbServers(info: ServerInfo): void {
    for (const url of info.connectUrls) this.#addServer(url, false);
    for (const url of info.connectUrlsTls) this.#addServer(url, true);
  }

  #addServer(entry: string, tls: boolean): void {
    let address: NatsAddress;
    try {
      address = parseNatsServer(entry, tls);
    } catch {
      return;
    }
    if (
      this.#servers.some(
        (server) =>
          server.hostname === address.hostname &&
          server.port === address.port &&
          server.tls === address.tls,
      )
    ) {
      return;
    }
    this.#servers.push(address);
  }

  #nextServer(): NatsAddress {
    const index = this.#serverIndex % this.#servers.length;
    this.#serverIndex++;
    return this.#servers[index]!;
  }

  // -- reading --------------------------------------------------------------

  async #nextCommand(timeout: number): Promise<ServerCommand | null> {
    const deadline = Number.isFinite(timeout) ? Date.now() + timeout : Number.POSITIVE_INFINITY;
    for (;;) {
      const command = this.#pending.shift();
      if (command !== undefined) return command;
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new NatsError("the server did not answer in time", { code: NatsErrorCode.Timeout });
      }
      const chunk = await this.#readWithDeadline(remaining);
      if (chunk === null) return null;
      this.#lastRead = Date.now();
      const { commands, rest } = drain(concat(this.#buffer, chunk));
      this.#buffer = rest;
      this.#pending.push(...commands);
    }
  }

  /**
   * `transport.read()`, with the wait bounded.
   *
   * The deadline has to race the read, not merely be checked between reads: a
   * server that accepts the socket and then sends nothing leaves the read pending
   * forever, so a timeout that only guards the loop never fires.
   */
  #readWithDeadline(remaining: number): Promise<Uint8Array | null> {
    const transport = this.#transport;
    if (transport === null) return Promise.resolve(null);
    if (!Number.isFinite(remaining)) return transport.read();
    return new Promise<Uint8Array | null>((resolve, reject) => {
      const timer = setTimeout(
        () =>
          reject(
            new NatsError("the server did not answer in time", { code: NatsErrorCode.Timeout }),
          ),
        remaining,
      );
      unrefTimer(timer as unknown as number);
      transport.read().then(
        (chunk) => {
          clearTimeout(timer);
          resolve(chunk);
        },
        (error) => {
          clearTimeout(timer);
          reject(error);
        },
      );
    });
  }

  async #readLoop(): Promise<void> {
    try {
      for (;;) {
        const command = await this.#nextCommand(Number.POSITIVE_INFINITY);
        if (command === null) {
          if (!this.#closing) {
            throw new NatsError("the server closed the connection", {
              code: NatsErrorCode.Connection,
            });
          }
          return;
        }
        this.#dispatch(command);
      }
    } catch (e) {
      if (!this.#closing && this.#failure === null) this.#lost(asNatsError(e));
    }
  }

  #dispatch(command: ServerCommand): void {
    switch (command.kind) {
      case "info":
        this.#info = command.info;
        this.#absorbServers(command.info);
        break;
      case "msg":
        this.#handlers.onMessage(this.#toIncoming(command));
        break;
      case "ping":
        void this.#write(encodePong()).catch(() => {});
        break;
      case "pong":
        this.#settlePong();
        break;
      case "ok":
        break;
      case "err":
        this.#fail(this.#serverError(command.message, NatsErrorCode.Server));
        break;
    }
  }

  #toIncoming(message: Extract<ServerCommand, { kind: "msg" }>): IncomingMessage {
    const decoded =
      message.headers === null ? { headers: {}, status: null } : decodeHeaders(message.headers);
    return {
      subject: message.subject,
      sid: message.sid,
      reply: message.reply,
      payload: message.payload,
      headers: decoded.headers,
      status: decoded.status,
    };
  }

  #serverError(message: string, code: NatsErrorCode): NatsError {
    const auth = /authoriz|authenticat/i.test(message);
    return new NatsError(`the server reported: ${message}`, {
      code: auth ? NatsErrorCode.Auth : code,
      serverError: message,
    });
  }

  // -- writing --------------------------------------------------------------

  async #writeRaw(bytes: Uint8Array): Promise<void> {
    const transport = this.#transport;
    if (transport === null) {
      throw new NatsError("the connection is not open", { code: NatsErrorCode.Connection });
    }
    await transport.write(bytes);
  }

  /**
   * Writes bytes, waiting first for the connection to be up.
   *
   * A write issued while the connection is down waits for the reconnect without
   * taking a place in the serialised chain: it joins the chain only once it is
   * up, so the resubscriptions a reconnect sends (which happen before the gate is
   * opened) always go out ahead of a caller's late publish, not behind it.
   */
  async #write(bytes: Uint8Array): Promise<void> {
    if (this.#failure !== null) throw this.#failure;
    if (this.#closed) {
      throw new NatsError("the connection is closed", { code: NatsErrorCode.Closed });
    }
    if (!this.#connected) {
      await this.#ready;
      if (this.#failure !== null) throw this.#failure;
      if (this.#closed) {
        throw new NatsError("the connection is closed", { code: NatsErrorCode.Closed });
      }
    }
    return this.#enqueue(bytes);
  }

  #enqueue(bytes: Uint8Array): Promise<void> {
    this.#pendingWrites++;
    const next = this.#writeChain.then(() => {
      if (this.#failure !== null) throw this.#failure;
      if (this.#closed) {
        throw new NatsError("the connection is closed", { code: NatsErrorCode.Closed });
      }
      const transport = this.#transport;
      if (transport === null) {
        throw new NatsError("the connection is not open", { code: NatsErrorCode.Connection });
      }
      return transport.write(bytes);
    });
    this.#writeChain = next.then(
      () => {
        this.#pendingWrites--;
      },
      () => {
        this.#pendingWrites--;
      },
    );
    return next.catch((e) => {
      if (this.#failure !== null) throw this.#failure;
      if (this.#closing) {
        throw new NatsError("the connection was closed during a write", {
          code: NatsErrorCode.Closed,
          cause: e,
        });
      }
      throw this.#lost(asNatsError(e));
    });
  }

  /** Publishes a message, without waiting for anything but the socket. */
  async publish(
    subject: string,
    payload: string | Uint8Array,
    options: {
      reply?: string;
      headers?: Record<string, string | readonly string[]>;
      status?: HeaderStatus | number;
    } = {},
  ): Promise<void> {
    validateSubject(subject, false);
    if (options.reply !== undefined) validateSubject(options.reply, false);
    const body = typeof payload === "string" ? new TextEncoder().encode(payload) : payload;
    const reply = options.reply ?? null;
    if (options.headers !== undefined || options.status !== undefined) {
      const headerBytes = encodeHeaders(options.headers ?? {}, options.status);
      this.#checkPayload(headerBytes.length + body.length);
      return this.#write(encodeHpub(subject, reply, headerBytes, body));
    }
    this.#checkPayload(body.length);
    return this.#write(encodePub(subject, reply, body));
  }

  /**
   * Registers a subscription and sends `SUB`.
   *
   * The id is allocated here and kept with the subject, so a reconnect can
   * re-establish the subscription under the same id and the server's `MSG` lines
   * keep naming a subscription the client still knows.
   */
  subscribe(subject: string, queue?: string): number {
    validateSubject(subject, true);
    if (queue !== undefined) validateQueue(queue);
    const sid = this.#nextSid;
    this.#nextSid = this.#nextSid >= 2_147_483_647 ? 1 : this.#nextSid + 1;
    this.#subscriptions.set(sid, queue === undefined ? { subject } : { subject, queue });
    if (this.#connected) {
      void this.#write(encodeSub(subject, queue, sid)).catch(() => {});
    }
    return sid;
  }

  /** Removes a subscription and sends `UNSUB`. */
  unsubscribe(sid: number, max?: number): void {
    this.#subscriptions.delete(sid);
    if (this.#connected) {
      void this.#write(encodeUnsub(sid, max)).catch(() => {});
    }
  }

  /**
   * Sends a `PING` and resolves when its `PONG` arrives.
   *
   * `PONG`s carry no identifier and the server answers in order, so callers are
   * settled first-in-first-out. A caller that times out is settled, but its place
   * in the queue is **kept**: the `PONG` that belongs to its `PING` is still on
   * the wire, and consuming that slot here is what stops it from settling the
   * next flush. (A later loss clears the queue whole.)
   */
  flush(timeout: number = DEFAULT_TIMEOUT): Promise<void> {
    if (this.#failure !== null) return Promise.reject(this.#failure);
    if (this.#closed) {
      return Promise.reject(
        new NatsError("the connection is closed", { code: NatsErrorCode.Closed }),
      );
    }
    return new Promise<void>((resolve, reject) => {
      const waiter: PingWaiter = { resolve, reject, timer: null };
      const timer = setTimeout(() => {
        // Do not remove the waiter: mark it dead and leave it queued, so the
        // PONG it is owed is consumed by it rather than by the next flush.
        if (waiter.timer !== null) clearTimeout(waiter.timer);
        waiter.timer = null;
        waiter.resolve = null;
        waiter.reject = null;
        reject(new NatsError("no PONG came back in time", { code: NatsErrorCode.Timeout }));
      }, timeout);
      unrefTimer(timer as unknown as number);
      waiter.timer = timer;
      this.#pings.push(waiter);
      void this.#write(encodePing()).catch((e) => {
        this.#removePing(waiter);
        reject(asNatsError(e));
      });
    });
  }

  #removePing(waiter: PingWaiter): void {
    const at = this.#pings.indexOf(waiter);
    if (at >= 0) this.#pings.splice(at, 1);
    if (waiter.timer !== null) clearTimeout(waiter.timer);
  }

  #settlePong(): void {
    const waiter = this.#pings.shift();
    if (waiter === undefined) return;
    if (waiter.timer !== null) clearTimeout(waiter.timer);
    if (waiter.resolve !== null) waiter.resolve();
  }

  #rejectPings(error: NatsError): void {
    for (const waiter of this.#pings) {
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      if (waiter.reject !== null) waiter.reject(error);
    }
    this.#pings = [];
  }

  /**
   * Closes the connection. There is no goodbye command in core NATS, so this is
   * the socket — aborted if a write is still pending, so close cannot hang
   * behind it.
   */
  async close(): Promise<void> {
    if (this.#closing || this.#failure !== null) {
      await this.#settle();
      this.#closed = true;
      return;
    }
    this.#closing = true;
    this.#connected = false;
    this.#stopKeepAlive();
    const reason = new NatsError("the connection was closed", { code: NatsErrorCode.Closed });
    this.#rejectPings(reason);
    // A write issued while the connection was down is parked on the readiness
    // gate. Closing stops the reconnect loop that would otherwise settle it, so
    // the gate is rejected here — otherwise that write would hang forever behind
    // a reconnect that is never coming.
    this.#rejectReady(reason);
    if (this.#pendingWrites > 0) {
      this.#closed = true;
      await this.#transport?.close({ abort: true }).catch(() => {});
      await this.#settle();
      return;
    }
    await this.#transport?.close().catch(() => {});
    await this.#settle();
    this.#closed = true;
  }

  /** Fails the connection from outside the read loop, latching the cause. */
  abort(error: NatsError): void {
    this.#fail(error);
  }

  // -- keep-alive -----------------------------------------------------------

  #armKeepAlive(): void {
    const interval = this.#settings.pingInterval ?? DEFAULT_PING_INTERVAL;
    if (interval <= 0) return;
    const tick = () => {
      if (this.#failure !== null || this.#closing) return;
      if (Date.now() - this.#lastRead > interval * 2) {
        this.#lost(
          new NatsError(
            `the server sent nothing for ${interval * 2}ms — treating the connection as lost`,
            { code: NatsErrorCode.Connection },
          ),
        );
        return;
      }
      this.#pings.push({ resolve: null, reject: null, timer: null });
      void this.#write(encodePing()).catch(() => {});
      this.#keepAliveTimer = setTimeout(tick, interval);
      unrefTimer(this.#keepAliveTimer as unknown as number);
    };
    this.#keepAliveTimer = setTimeout(tick, interval);
    unrefTimer(this.#keepAliveTimer as unknown as number);
  }

  #stopKeepAlive(): void {
    if (this.#keepAliveTimer !== null) {
      clearTimeout(this.#keepAliveTimer);
      this.#keepAliveTimer = null;
    }
  }

  // -- loss and recovery ----------------------------------------------------

  #reconnectSettings(): ReconnectSettings {
    const value = this.#settings.reconnect;
    if (value === false) return { enabled: false, maxAttempts: 0, wait: 0 };
    if (value === true || value === undefined) {
      return {
        enabled: true,
        maxAttempts: DEFAULT_MAX_RECONNECT_ATTEMPTS,
        wait: DEFAULT_RECONNECT_WAIT,
      };
    }
    return {
      enabled: true,
      maxAttempts: value.maxAttempts ?? DEFAULT_MAX_RECONNECT_ATTEMPTS,
      wait: value.wait ?? DEFAULT_RECONNECT_WAIT,
    };
  }

  #lost(error: NatsError): NatsError {
    if (this.#closing || this.#closed) return error;
    if (this.#reconnecting) return error;
    this.#connected = false;
    this.#rejectPings(error);
    this.#stopKeepAlive();
    this.#handlers.onDisconnect(error);
    const reconnect = this.#reconnectSettings();
    if (!reconnect.enabled) {
      this.#fail(error);
      return error;
    }
    this.#reconnecting = true;
    this.#armReady();
    void this.#reconnect(error);
    return error;
  }

  async #reconnect(initial: NatsError): Promise<void> {
    const { maxAttempts, wait } = this.#reconnectSettings();
    let last = initial;
    for (let attempt = 1; attempt <= maxAttempts; attempt++) {
      if (this.#closing || this.#closed) return;
      if (wait > 0) await delay(wait);
      if (this.#closing || this.#closed) return;
      const address = this.#nextServer();
      try {
        await this.#dialTo(address);
      } catch (e) {
        last = asNatsError(e);
        continue;
      }
      this.#resubscribe();
      this.#reconnecting = false;
      this.#resolveReady();
      this.#handlers.onReconnect();
      return;
    }
    this.#reconnecting = false;
    this.#fail(
      new NatsError(`the connection could not be re-established after ${maxAttempts} attempts`, {
        code: NatsErrorCode.Connection,
        cause: last,
      }),
    );
  }

  async #dialTo(address: NatsAddress): Promise<void> {
    const transport = await this.#dial({
      hostname: address.hostname,
      port: address.port,
      tls: address.tls,
      ...(this.#settings.ca === undefined ? {} : { ca: this.#settings.ca }),
      ...(this.#settings.servername === undefined ? {} : { servername: this.#settings.servername }),
    });
    await this.#attach(transport, address);
  }

  #resubscribe(): void {
    for (const [sid, subscription] of this.#subscriptions) {
      void this.#write(encodeSub(subscription.subject, subscription.queue, sid)).catch(() => {});
    }
  }

  #armReady(): void {
    this.#ready = new Promise<void>((resolve, reject) => {
      this.#readyResolve = resolve;
      this.#readyReject = reject;
    });
    void this.#ready.catch(() => {});
  }

  #resolveReady(): void {
    const resolve = this.#readyResolve;
    this.#readyResolve = () => {};
    this.#readyReject = () => {};
    this.#ready = Promise.resolve();
    resolve();
  }

  /** Fails a parked readiness gate, so a write waiting on a reconnect gives up. */
  #rejectReady(error: NatsError): void {
    const reject = this.#readyReject;
    this.#readyResolve = () => {};
    this.#readyReject = () => {};
    reject(error);
  }

  // -- failure --------------------------------------------------------------

  #fail(error: NatsError): NatsError {
    if (this.#failure === null) {
      this.#failure = error;
      this.#closed = true;
      this.#connected = false;
      this.#rejectPings(error);
      this.#stopKeepAlive();
      this.#rejectReady(error);
      void this.#transport?.close({ abort: true }).catch(() => {});
      this.#handlers.onClosed(error);
    }
    return this.#failure;
  }

  async #settle(): Promise<void> {
    if (this.#pump !== null) await this.#pump.catch(() => {});
  }

  #checkPayload(size: number): void {
    if (size > this.maxPayload) {
      throw new NatsError(
        `the message is ${size} bytes, over the server's ${this.maxPayload}-byte maximum`,
        { code: NatsErrorCode.Payload },
      );
    }
  }
}

/** A dialler: something that turns an address into a {@link Transport}. */
export type Dialer = (options: NetTransportOptions) => Promise<Transport>;

/** Opens a connection to `url` and completes the handshake. */
export function openConnection(
  url: string,
  settings: ConnectionSettings = {},
  handlers: Partial<ConnectionHandlers> = {},
): Promise<Connection> {
  return Connection.open(url, settings, handlers);
}

/**
 * {@link Connection.openWith}, as a free function.
 *
 * The dialler is the seam a test stands on: a substituted one can hand over a
 * scripted server, and a list can name a second address to fail over to.
 */
export function openWithDial(
  url: string,
  settings: ConnectionSettings = {},
  handlers: Partial<ConnectionHandlers> = {},
  dial: Dialer = netTransport,
): Promise<Connection> {
  return Connection.openWith(url, settings, handlers, dial);
}
