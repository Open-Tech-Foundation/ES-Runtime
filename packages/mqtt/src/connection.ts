/**
 * The connection: one socket, one MQTT session, and the framing and keep-alive
 * that keep it honest.
 *
 * Three things this layer owns and no layer above it should have to:
 *
 * - **The packet boundary.** MQTT is a length-prefixed stream, and a socket
 *   delivers it in whatever chunks it likes. This layer accumulates bytes and is
 *   what decides a packet is complete — a fixed header that arrives alone, a
 *   Remaining Length split across two reads, several packets in one chunk are
 *   all its problem, solved once, rather than every packet's.
 * - **The protocol's own answers.** A QoS 1 publish in either direction is
 *   acknowledged with PUBACK; a QoS 2 exchange runs PUBREC → PUBREL → PUBCOMP,
 *   whichever side sent the message. The acknowledgements that need no
 *   application decision are sent here, and a QoS 2 message is delivered to the
 *   application once — on PUBREL, not on PUBLISH — so a retransmitted PUBLISH
 *   cannot become a second delivery.
 * - **Fatality.** A lost socket or bytes that are not MQTT leaves the stream at
 *   an unknown boundary, and every later packet would be misread. Such a failure
 *   is latched once and every caller gets it, rather than each seeing a
 *   different symptom of the one dead connection.
 *
 * What it deliberately does not own: subscriptions, request/reply, and
 * reconnection. The first two are the client's; deciding to reconnect is the
 * application's, which is why {@link MqttError} says what happened and the layer
 * does not quietly dial again.
 */

import { unrefTimer } from "runtime:process";
import { MqttError, MqttErrorCode, ProtocolError } from "./errors.js";
import {
  type AckPacket,
  type ConnackPacket,
  type ConnectPacket,
  type DisconnectPacket,
  decodePacketBody,
  encodePacket,
  type Packet,
  PacketType,
  type ProtocolLevel,
  type PublishPacket,
  packetName,
  type SubackPacket,
  type UnsubackPacket,
  type Will,
} from "./protocol/packets.js";
import type { Properties } from "./protocol/properties.js";
import { connack311Name, isErrorReason, reasonName } from "./protocol/reason.js";
import { netTransport, type Transport } from "./transport.js";

/** MQTT's default port for `mqtt://`. */
export const DEFAULT_MQTT_PORT = 1883;
/** MQTT's default port for `mqtts://`. */
export const DEFAULT_MQTTS_PORT = 8883;
/** The keep-alive this client asks for when nothing else is asked. */
export const DEFAULT_KEEP_ALIVE = 60;
/** How long the handshake and any one request may take, in ms. */
const DEFAULT_TIMEOUT = 30_000;

/** A reached broker, parsed out of a URL. */
export interface MqttAddress {
  hostname: string;
  port: number;
  tls: boolean;
  username?: string;
  password?: string;
}

function decodeComponent(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    throw new MqttError(`a URL component was not valid percent-encoding: ${value}`, {
      code: MqttErrorCode.Protocol,
    });
  }
}

/**
 * Parses an `mqtt://` or `mqtts://` URL into a host and credentials.
 *
 * The scheme picks the transport and the default port together: `mqtt` (and
 * `tcp`) mean plain on 1883, `mqtts` (and `ssl`, `tls`) mean TLS on 8883. A URL
 * with no port keeps the scheme's default, which is the rule that makes `mqtts://`
 * mean port 8883 and not 1883.
 */
export function parseMqttUrl(url: string): MqttAddress {
  const match = /^([a-z][a-z0-9+.-]*):\/\/(.*)$/i.exec(url);
  if (match === null) {
    throw new MqttError(`not a broker URL: ${JSON.stringify(url)}`, {
      code: MqttErrorCode.Protocol,
    });
  }
  const scheme = match[1]!.toLowerCase();
  const tls = scheme === "mqtts" || scheme === "ssl" || scheme === "tls";
  if (!["mqtt", "mqtts", "tcp", "ssl", "tls"].includes(scheme)) {
    throw new MqttError(`unknown broker scheme ${JSON.stringify(scheme)}`, {
      code: MqttErrorCode.Protocol,
    });
  }

  let rest = match[2]!;
  const slash = rest.indexOf("/");
  if (slash >= 0) rest = rest.slice(0, slash);

  let username: string | undefined;
  let password: string | undefined;
  const at = rest.lastIndexOf("@");
  if (at >= 0) {
    const userinfo = rest.slice(0, at);
    rest = rest.slice(at + 1);
    const colon = userinfo.indexOf(":");
    username = decodeComponent(colon < 0 ? userinfo : userinfo.slice(0, colon));
    if (colon >= 0) password = decodeComponent(userinfo.slice(colon + 1));
  }

  let hostname: string;
  let port: number;
  if (rest.startsWith("[")) {
    const close = rest.indexOf("]");
    if (close < 0)
      throw new MqttError(`an IPv6 host is missing its \`]\`: ${url}`, {
        code: MqttErrorCode.Protocol,
      });
    hostname = rest.slice(1, close);
    const after = rest.slice(close + 1);
    port = after.startsWith(":")
      ? Number(after.slice(1))
      : tls
        ? DEFAULT_MQTTS_PORT
        : DEFAULT_MQTT_PORT;
  } else {
    const colon = rest.lastIndexOf(":");
    if (colon >= 0) {
      hostname = rest.slice(0, colon);
      port = Number(rest.slice(colon + 1));
    } else {
      hostname = rest;
      port = tls ? DEFAULT_MQTTS_PORT : DEFAULT_MQTT_PORT;
    }
  }
  if (hostname.length === 0) {
    throw new MqttError(`the URL has no host: ${url}`, { code: MqttErrorCode.Protocol });
  }
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new MqttError(`the URL has an invalid port: ${url}`, { code: MqttErrorCode.Protocol });
  }

  return {
    hostname,
    port,
    tls,
    ...(username === undefined ? {} : { username }),
    ...(password === undefined ? {} : { password }),
  };
}

/** How to identify the client and what session to ask for. */
export interface ConnectionSettings {
  /** The client identifier. Empty or omitted means the broker assigns one (5.0). */
  clientId?: string;
  username?: string;
  password?: string | Uint8Array;
  /** Keep-alive in seconds. Defaults to {@link DEFAULT_KEEP_ALIVE}; `0` disables it. */
  keepAlive?: number;
  /** Whether to start a clean session. Defaults to `true`. */
  cleanStart?: boolean;
  /** The last will and testament. */
  will?: Will;
  /** Seconds the session survives a disconnect (5.0). */
  sessionExpiryInterval?: number;
  /** Extra CONNECT properties, merged over the ones derived from the fields above. */
  properties?: Properties;
  /**
   * The protocol level to use. Omitted means negotiate: try 5.0, fall back to
   * 3.1.1 when the broker will not speak 5.0.
   */
  protocolVersion?: ProtocolLevel;
  /** How long the handshake may take, in ms. Defaults 30 000. */
  timeout?: number;
  /** Trust anchors for `mqtts://`, as PEM. */
  ca?: string | Uint8Array;
  /** The name the TLS certificate must carry, when it is not the host. */
  servername?: string;
}

/**
 * What the connection tells the client.
 *
 * The connection answers the protocol's own acknowledgements itself, so what
 * arrives here is what needs an application decision: a message, an
 * acknowledgement of something the client sent, a SUBACK or UNSUBACK for a
 * request the client is waiting on, a PINGRESP, a broker's DISCONNECT, and the
 * one news that the connection is over.
 */
export interface ConnectionHandlers {
  onMessage(packet: PublishPacket): void;
  onAck(packet: AckPacket): void;
  onSuback(packet: SubackPacket): void;
  onUnsuback(packet: UnsubackPacket): void;
  onPingresp(): void;
  onDisconnect(packet: DisconnectPacket): void;
  onClosed(error: MqttError | null): void;
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

/** One in-flight PINGREQ/PINGRESP exchange, and everyone waiting on it. */
interface PingCycle {
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: MqttError) => void;
  timer: ReturnType<typeof setTimeout>;
}

/**
 * Reads a fixed header and Remaining Length from the front of `bytes`.
 *
 * Returns `null` when the buffer is too short to say — not an error, just a
 * packet not yet arrived — and throws only when the length itself is malformed
 * (a fifth octet), which no amount of waiting fixes.
 */
function readFrame(
  bytes: Uint8Array,
): { total: number; body: Uint8Array; firstByte: number } | null {
  if (bytes.length < 2) return null;
  let remaining = 0;
  let multiplier = 1;
  for (let i = 1; i <= 4; i++) {
    if (i >= bytes.length) return null;
    const octet = bytes[i]!;
    remaining += (octet & 0x7f) * multiplier;
    if ((octet & 0x80) === 0) {
      const headerLength = 1 + i;
      const total = headerLength + remaining;
      if (bytes.length < total) return null;
      return { total, body: bytes.subarray(headerLength, total), firstByte: bytes[0]! };
    }
    multiplier *= 128;
  }
  throw new ProtocolError("a Remaining Length of more than four octets is malformed");
}

export class Connection {
  readonly #transport: Transport;
  readonly #settings: ConnectionSettings;
  readonly #handlers: ConnectionHandlers;
  #level: ProtocolLevel;
  #keepAlive: number;
  #buffer: Uint8Array = new Uint8Array(0);

  #closing = false;
  #closed = false;
  #failure: MqttError | null = null;
  #pump: Promise<void> | null = null;
  #keepAliveTimer: ReturnType<typeof setTimeout> | null = null;
  #lastRead = Date.now();
  #writeChain: Promise<void> = Promise.resolve();
  #pendingWrites = 0;
  #nextPacketId = 1;
  #ping: PingCycle | null = null;
  #pendingQos2 = new Map<number, PublishPacket>();
  #connack: ConnackPacket;

  private constructor(
    transport: Transport,
    settings: ConnectionSettings,
    handlers: ConnectionHandlers,
    level: ProtocolLevel,
    connack: ConnackPacket,
  ) {
    this.#transport = transport;
    this.#settings = settings;
    this.#handlers = handlers;
    this.#level = level;
    this.#connack = connack;
    this.#keepAlive =
      level === 5 && connack.properties?.serverKeepAlive !== undefined
        ? connack.properties.serverKeepAlive
        : (settings.keepAlive ?? DEFAULT_KEEP_ALIVE);
  }

  /** What the broker said in its CONNACK. */
  get connack(): ConnackPacket {
    return this.#connack;
  }

  /** The negotiated protocol level: 5 for MQTT 5.0, 4 for 3.1.1. */
  get protocolLevel(): ProtocolLevel {
    return this.#level;
  }

  /** The keep-alive in force, in seconds; `0` when disabled. */
  get keepAlive(): number {
    return this.#keepAlive;
  }

  /** Whether the broker kept a session for this client identifier. */
  get sessionPresent(): boolean {
    return this.connack.sessionPresent;
  }

  /** Whether the connection is over — deliberately closed, or failed. */
  get closed(): boolean {
    return this.#closed;
  }

  /** The failure that ended the connection, or `null` while it stands. */
  get failure(): MqttError | null {
    return this.#failure;
  }

  /** The next unused packet identifier (1–65535, wrapping). */
  allocatePacketId(): number {
    const id = this.#nextPacketId;
    this.#nextPacketId = this.#nextPacketId >= 65535 ? 1 : this.#nextPacketId + 1;
    return id;
  }

  /** Sends one packet, serialised so a packet is never interleaved with another. */
  async send(packet: Packet): Promise<void> {
    await this.#write(encodePacket(packet, this.#level));
  }

  /**
   * Fails the connection from outside the read loop.
   *
   * Used when a caller gives up on a packet the connection may still be writing:
   * the packet could yet reach the broker, and the stuck write — held in the
   * serialised chain — would block every later packet, the final DISCONNECT
   * among them. Failing latches the cause, closes the socket and releases the
   * chain.
   */
  abort(error: MqttError): void {
    this.#fail(error);
  }

  /**
   * Sends a PINGREQ and resolves when the PINGRESP arrives.
   *
   * A PINGRESP carries no identifier, so two PINGREQs in flight are
   * indistinguishable: one reply answers either, and a reply to a ping already
   * given up on would answer the next one. So there is at most one exchange at a
   * time — concurrent callers share it — and a timeout is treated as a lost
   * connection, because after it the next PINGRESP can no longer be attributed.
   */
  async ping(timeout: number = DEFAULT_TIMEOUT): Promise<void> {
    if (this.#failure !== null) throw this.#failure;
    if (this.#closed) {
      throw new MqttError("the connection is closed", { code: MqttErrorCode.Closed });
    }
    if (this.#ping !== null) return this.#ping.promise;

    let resolve!: () => void;
    let reject!: (error: MqttError) => void;
    const promise = new Promise<void>((res, rej) => {
      resolve = res;
      reject = rej;
    });
    // The timer is the failure: a PINGRESP that never came leaves the exchange
    // uncorrelated, so the connection is failed rather than left half-alive.
    const timer = setTimeout(
      () =>
        this.#fail(
          new MqttError("no PINGRESP came back in time", { code: MqttErrorCode.Connection }),
        ),
      timeout,
    );
    unrefTimer(timer as unknown as number);
    this.#ping = { promise, resolve, reject, timer };
    // Kept handled so a caller that never awaits — because the send throws
    // first — cannot trip the runtime's unhandled-rejection guard.
    void promise.catch(() => {});

    try {
      await this.send({ type: PacketType.Pingreq });
    } catch (e) {
      const error =
        e instanceof MqttError
          ? e
          : new MqttError(e instanceof Error ? e.message : String(e), {
              code: MqttErrorCode.Connection,
              cause: e,
            });
      this.#settlePing(error);
      throw error;
    }
    return promise;
  }

  /** Settles the in-flight ping, if any: resolve on a PINGRESP, else reject. */
  #settlePing(error: MqttError | null): void {
    const cycle = this.#ping;
    if (cycle === null) return;
    this.#ping = null;
    clearTimeout(cycle.timer);
    if (error === null) cycle.resolve();
    else cycle.reject(error);
  }

  /**
   * Closes the connection, then waits for the read loop to end. If a write is
   * pending, the transport is aborted because DISCONNECT would queue behind a
   * write that may never complete.
   */
  async close(options: { reasonCode?: number; properties?: Properties } = {}): Promise<void> {
    if (this.#closing || this.#failure !== null) {
      await this.#settle();
      this.#closed = true;
      this.#settlePing(new MqttError("the connection was closed", { code: MqttErrorCode.Closed }));
      return;
    }
    this.#closing = true;
    // Settled before the DISCONNECT so the ping timer cannot fire mid-close.
    this.#settlePing(new MqttError("the connection was closed", { code: MqttErrorCode.Closed }));
    if (this.#pendingWrites > 0) {
      // DISCONNECT would be queued behind a write that may never complete. Tear
      // down the transport deliberately instead: the caller requested closure,
      // so this is not a connection failure and must not latch `failure`.
      this.#closed = true;
      await this.#transport.close({ abort: true }).catch(() => {});
      await this.#settle();
      this.#stopKeepAlive();
      return;
    }
    try {
      await this.send({
        type: PacketType.Disconnect,
        reasonCode: options.reasonCode ?? 0,
        ...(options.properties === undefined ? {} : { properties: options.properties }),
      });
    } catch {
      /* the socket may already be gone; closing still has to finish */
    }
    await this.#transport.close().catch(() => {});
    await this.#settle();
    this.#stopKeepAlive();
    this.#closed = true;
  }

  // -- the opening handshake ------------------------------------------------

  /**
   * Opens a connection over an already-established transport.
   *
   * This is the seam a test stands on: the handshake and the read loop are the
   * same whether the bytes come from a socket or a scripted broker in memory,
   * so the version negotiation and the packet routing can be exercised with no
   * port open and no capability granted.
   */
  static async over(
    transport: Transport,
    settings: ConnectionSettings = {},
    handlers: Partial<ConnectionHandlers> = {},
    level: ProtocolLevel = 5,
  ): Promise<Connection> {
    const full = Connection.#handlersOf(handlers);
    const connection = new Connection(transport, settings, full, level, {
      type: PacketType.Connack,
      sessionPresent: false,
      reasonCode: 0,
    });
    const connack = await connection.#handshake(level);
    if (isErrorReason(connack.reasonCode)) {
      const name =
        level === 5 ? reasonName(connack.reasonCode) : connack311Name(connack.reasonCode);
      throw new MqttError(
        `the broker refused the connection: ${name ?? `reason ${connack.reasonCode}`}`,
        {
          code:
            connack.reasonCode === 0x87 || connack.reasonCode === 0x86
              ? MqttErrorCode.Auth
              : MqttErrorCode.Refused,
          reasonCode: connack.reasonCode,
          ...(name === null ? {} : { reasonName: name }),
        },
      );
    }
    // A non-zero code below 0x80 is not a valid 5.0 CONNACK success code — it is
    // an older broker's return code read as one — so it is a version mismatch.
    if (level === 5 && connack.reasonCode !== 0) {
      throw new MqttError("the broker did not accept MQTT 5.0", { code: MqttErrorCode.Handshake });
    }
    return connection.#start(level, connack);
  }

  static #handlersOf(handlers: Partial<ConnectionHandlers>): ConnectionHandlers {
    return {
      onMessage: handlers.onMessage ?? (() => {}),
      onAck: handlers.onAck ?? (() => {}),
      onSuback: handlers.onSuback ?? (() => {}),
      onUnsuback: handlers.onUnsuback ?? (() => {}),
      onPingresp: handlers.onPingresp ?? (() => {}),
      onDisconnect: handlers.onDisconnect ?? (() => {}),
      onClosed: handlers.onClosed ?? (() => {}),
    };
  }

  /**
   * Opens a connection to `url`: dial the broker, run the handshake, start the
   * read loop.
   *
   * The version negotiation is here and nowhere else. A broker that will not
   * speak 5.0 says so one of two ways — a CONNACK reason code of 0x84, or an
   * older CONNACK whose return code has no 5.0 meaning — and both mean the same
   * thing: dial again as 3.1.1. Any other refusal (bad credentials, a banned
   * client) is reported, not retried, because a second protocol level would not
   * change it.
   */
  static open(
    url: string,
    settings: ConnectionSettings = {},
    handlers: Partial<ConnectionHandlers> = {},
  ): Promise<Connection> {
    return openWithDial(url, settings, handlers, netTransport);
  }

  #start(level: ProtocolLevel, connack: ConnackPacket): Connection {
    this.#connack = connack;
    this.#level = level;
    this.#keepAlive =
      level === 5 && connack.properties?.serverKeepAlive !== undefined
        ? connack.properties.serverKeepAlive
        : (this.#settings.keepAlive ?? DEFAULT_KEEP_ALIVE);
    this.#pump = this.#readLoop();
    this.#armKeepAlive();
    return this;
  }

  async #handshake(level: ProtocolLevel): Promise<ConnackPacket> {
    const timeout = this.#settings.timeout ?? DEFAULT_TIMEOUT;
    const connect = this.#connectPacket();
    await this.#write(encodePacket(connect, level));
    const packet = await this.#readPacket(level, timeout);
    if (packet === null) {
      throw new MqttError("the broker closed the connection during the handshake", {
        code: MqttErrorCode.Connection,
      });
    }
    if (packet.type !== PacketType.Connack) {
      throw new ProtocolError("the first packet from the broker was not a CONNACK");
    }
    return packet;
  }

  #connectPacket(): ConnectPacket {
    const will = this.#settings.will;
    const password = this.#settings.password;
    const properties = this.#connectProperties();
    return {
      type: PacketType.Connect,
      clientId: this.#settings.clientId ?? "",
      ...(this.#settings.username === undefined ? {} : { username: this.#settings.username }),
      ...(password === undefined
        ? {}
        : {
            password: typeof password === "string" ? new TextEncoder().encode(password) : password,
          }),
      keepAlive: this.#settings.keepAlive ?? DEFAULT_KEEP_ALIVE,
      cleanStart: this.#settings.cleanStart ?? true,
      ...(will === undefined ? {} : { will }),
      ...(properties === undefined ? {} : { properties }),
    };
  }

  #connectProperties(): Properties | undefined {
    const extra = this.#settings.properties;
    const sessionExpiry = this.#settings.sessionExpiryInterval;
    if (sessionExpiry === undefined && extra === undefined) return undefined;
    return {
      ...(extra ?? {}),
      ...(sessionExpiry === undefined ? {} : { sessionExpiryInterval: sessionExpiry }),
    };
  }

  // -- the read loop --------------------------------------------------------

  async #readPacket(level: ProtocolLevel, timeout: number): Promise<Packet | null> {
    const deadline = Number.isFinite(timeout) ? Date.now() + timeout : Number.POSITIVE_INFINITY;
    for (;;) {
      const frame = readFrame(this.#buffer);
      if (frame !== null) {
        this.#buffer = this.#buffer.subarray(frame.total);
        return decodePacketBody(frame.firstByte, frame.body, level);
      }
      const remaining = deadline - Date.now();
      if (remaining <= 0) {
        throw new MqttError("the broker did not answer in time", { code: MqttErrorCode.Timeout });
      }
      const chunk = await this.#readWithDeadline(remaining);
      if (chunk === null) return null;
      this.#buffer = concat(this.#buffer, chunk);
    }
  }

  /**
   * `transport.read()`, with the wait bounded.
   *
   * The deadline has to race the read, not merely be checked between reads: a
   * broker that accepts the socket and then sends nothing leaves the read
   * pending forever, so a timeout that only guards the loop never fires. The
   * abandoned read is harmless — it settles onto a closed stream — but its
   * result is ignored, so the caller hears the timeout it asked for.
   */
  #readWithDeadline(remaining: number): Promise<Uint8Array | null> {
    if (!Number.isFinite(remaining)) return this.#transport.read();
    return new Promise<Uint8Array | null>((resolve, reject) => {
      const timer = setTimeout(
        () =>
          reject(
            new MqttError("the broker did not answer in time", { code: MqttErrorCode.Timeout }),
          ),
        remaining,
      );
      unrefTimer(timer as unknown as number);
      this.#transport.read().then(
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
        const packet = await this.#readPacket(this.#level, Number.POSITIVE_INFINITY);
        if (packet === null) {
          if (!this.#closing) {
            throw new MqttError("the broker closed the connection", {
              code: MqttErrorCode.Connection,
            });
          }
          return;
        }
        this.#lastRead = Date.now();
        this.#dispatch(packet);
      }
    } catch (e) {
      if (!this.#closing) {
        this.#fail(
          e instanceof MqttError
            ? e
            : new MqttError(e instanceof Error ? e.message : String(e), {
                code: MqttErrorCode.Connection,
                cause: e,
              }),
        );
      }
    }
  }

  #dispatch(packet: Packet): void {
    switch (packet.type) {
      case PacketType.Publish:
        this.#onPublish(packet);
        break;
      case PacketType.Puback:
        this.#handlers.onAck(packet);
        break;
      case PacketType.Pubrec:
        // Step two of an outbound QoS 2 exchange. A successful PUBREC is answered
        // with PUBREL and the exchange waits on PUBCOMP; a negative one ends it
        // here — the specification sends no PUBREL and frees the packet
        // identifier — so it is reported at once rather than after a timeout.
        if (!isErrorReason(packet.reasonCode)) {
          void this.send({
            type: PacketType.Pubrel,
            packetId: packet.packetId,
            reasonCode: 0,
          }).catch(() => {});
        }
        this.#handlers.onAck(packet);
        break;
      case PacketType.Pubrel:
        // Step three of an inbound QoS 2 exchange: the broker has our PUBREC and
        // is releasing the message — this is where it is delivered, once.
        this.#completeInboundQos2(packet.packetId);
        break;
      case PacketType.Pubcomp:
        this.#handlers.onAck(packet);
        break;
      case PacketType.Suback:
        this.#handlers.onSuback(packet);
        break;
      case PacketType.Unsuback:
        this.#handlers.onUnsuback(packet);
        break;
      case PacketType.Pingresp:
        this.#settlePing(null);
        this.#handlers.onPingresp();
        break;
      case PacketType.Disconnect:
        this.#handlers.onDisconnect(packet);
        this.#fail(
          new MqttError(
            `the broker disconnected the client: ${reasonName(packet.reasonCode) ?? `reason ${packet.reasonCode}`}`,
            {
              code: packet.reasonCode === 0 ? MqttErrorCode.Closed : MqttErrorCode.Server,
              reasonCode: packet.reasonCode,
              ...(reasonName(packet.reasonCode) === null
                ? {}
                : { reasonName: reasonName(packet.reasonCode)! }),
            },
          ),
        );
        break;
      case PacketType.Auth:
      case PacketType.Connack:
      case PacketType.Connect:
      case PacketType.Subscribe:
      case PacketType.Unsubscribe:
      case PacketType.Pingreq:
        throw new ProtocolError(
          `the broker sent a ${packetName(packet.type)} packet it should not`,
        );
    }
  }

  #onPublish(packet: PublishPacket): void {
    if (packet.qos === 0) {
      this.#handlers.onMessage(packet);
      return;
    }
    if (packet.qos === 1) {
      void this.send({ type: PacketType.Puback, packetId: packet.packetId!, reasonCode: 0 }).catch(
        () => {},
      );
      this.#handlers.onMessage(packet);
      return;
    }
    // QoS 2: remember the message against its identifier and acknowledge the
    // receipt; it is delivered when the broker releases it with PUBREL, so a
    // retransmitted PUBLISH is stored, not delivered twice.
    this.#pendingQos2.set(packet.packetId!, packet);
    void this.send({ type: PacketType.Pubrec, packetId: packet.packetId!, reasonCode: 0 }).catch(
      () => {},
    );
  }

  #completeInboundQos2(packetId: number): void {
    const packet = this.#pendingQos2.get(packetId);
    this.#pendingQos2.delete(packetId);
    void this.send({ type: PacketType.Pubcomp, packetId, reasonCode: 0 }).catch(() => {});
    if (packet !== undefined) this.#handlers.onMessage(packet);
  }

  // -- writing --------------------------------------------------------------

  #write(bytes: Uint8Array): Promise<void> {
    // A failed connection takes no more packets. Rejecting here rather than
    // joining the chain matters because a write that never finishes would
    // otherwise hold every later writer behind it.
    if (this.#failure !== null) return Promise.reject(this.#failure);
    if (this.#closed) {
      return Promise.reject(
        new MqttError("the connection is closed", { code: MqttErrorCode.Closed }),
      );
    }
    this.#pendingWrites++;
    const next = this.#writeChain.then(() => {
      if (this.#failure !== null) throw this.#failure;
      if (this.#closed) {
        throw new MqttError("the connection is closed", { code: MqttErrorCode.Closed });
      }
      return this.#transport.write(bytes);
    });
    // Keep the chain alive after a failure so later writes see the latch, not an
    // unhandled rejection.
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
        throw new MqttError("the connection was closed during a write", {
          code: MqttErrorCode.Closed,
          cause: e,
        });
      }
      throw this.#fail(
        e instanceof MqttError
          ? e
          : new MqttError(e instanceof Error ? e.message : String(e), {
              code: MqttErrorCode.Connection,
              cause: e,
            }),
      );
    });
  }

  // -- keep-alive -----------------------------------------------------------

  #armKeepAlive(): void {
    if (this.#keepAlive <= 0) return;
    const every = Math.max(1000, (this.#keepAlive * 1000) / 2);
    const tick = () => {
      if (this.#failure !== null || this.#closing) return;
      if (Date.now() - this.#lastRead > this.#keepAlive * 2000) {
        this.#fail(
          new MqttError(
            `the broker sent nothing for ${this.#keepAlive * 2}s — treating the connection as lost`,
            { code: MqttErrorCode.Connection },
          ),
        );
        return;
      }
      // A caller's ping is already probing liveness; a second PINGREQ would be
      // answered by the same uncorrelated PINGRESP.
      if (this.#ping === null) void this.send({ type: PacketType.Pingreq }).catch(() => {});
      this.#keepAliveTimer = setTimeout(tick, every);
      unrefTimer(this.#keepAliveTimer as unknown as number);
    };
    this.#keepAliveTimer = setTimeout(tick, every);
    unrefTimer(this.#keepAliveTimer as unknown as number);
  }

  #stopKeepAlive(): void {
    if (this.#keepAliveTimer !== null) {
      clearTimeout(this.#keepAliveTimer);
      this.#keepAliveTimer = null;
    }
  }

  // -- failure --------------------------------------------------------------

  #fail(error: MqttError): MqttError {
    if (this.#failure === null) {
      this.#failure = error;
      this.#closed = true;
      this.#settlePing(error);
      this.#stopKeepAlive();
      void this.#transport.close({ abort: true }).catch(() => {});
      this.#handlers.onClosed(error);
    }
    return this.#failure;
  }

  async #settle(): Promise<void> {
    if (this.#pump !== null) await this.#pump.catch(() => {});
  }
}

/** Opens a connection to `url` and completes the handshake. */
export function openConnection(
  url: string,
  settings: ConnectionSettings = {},
  handlers: Partial<ConnectionHandlers> = {},
): Promise<Connection> {
  return Connection.open(url, settings, handlers);
}

/**
 * How a connection reaches a broker's socket. The default is
 * {@link netTransport}; a test substitutes a scripted broker so the version
 * negotiation can be exercised without a port.
 */
export type Dialer = (options: Parameters<typeof netTransport>[0]) => Promise<Transport>;

/**
 * {@link Connection.open}, with the dialler made explicit.
 *
 * The other seam a test stands on: it needs a second, different broker to fall
 * back to, and only a substituted dialler can hand one over. The negotiation
 * itself is the same loop `open` runs.
 */
export async function openWithDial(
  url: string,
  settings: ConnectionSettings = {},
  handlers: Partial<ConnectionHandlers> = {},
  dial: Dialer = netTransport,
): Promise<Connection> {
  const address = parseMqttUrl(url);
  const effective: ConnectionSettings = {
    ...settings,
    ...(settings.username === undefined && address.username !== undefined
      ? { username: address.username }
      : {}),
    ...(settings.password === undefined && address.password !== undefined
      ? { password: address.password }
      : {}),
  };
  const levels: ProtocolLevel[] =
    settings.protocolVersion === undefined ? [5, 4] : [settings.protocolVersion];

  let lastError: MqttError | null = null;
  for (let i = 0; i < levels.length; i++) {
    const level = levels[i]!;
    let transport: Transport;
    try {
      transport = await dial({
        hostname: address.hostname,
        port: address.port,
        tls: address.tls,
        ...(settings.ca === undefined ? {} : { ca: settings.ca }),
        ...(settings.servername === undefined ? {} : { servername: settings.servername }),
      });
    } catch (e) {
      throw e instanceof MqttError
        ? e
        : new MqttError(String(e), { code: MqttErrorCode.Connection });
    }

    try {
      return await Connection.over(transport, effective, handlers, level);
    } catch (e) {
      // Abandoning this socket, not closing it politely: the handshake may have
      // timed out with a write still pending, and a graceful close would wait on
      // that write rather than tear the socket down.
      await transport.close({ abort: true }).catch(() => {});
      const error =
        e instanceof MqttError
          ? e
          : new MqttError(e instanceof Error ? e.message : String(e), {
              code: MqttErrorCode.Handshake,
              cause: e,
            });
      // A protocol error or an early close while offering 5.0 is how a
      // 3.1.1-only broker usually answers; try the older level once.
      const worthRetrying =
        i < levels.length - 1 &&
        (error.reasonCode === 0x84 ||
          error.code === MqttErrorCode.Handshake ||
          error.code === MqttErrorCode.Protocol ||
          error.code === MqttErrorCode.Connection);
      if (worthRetrying) {
        lastError = error;
        continue;
      }
      throw error;
    }
  }
  throw lastError ?? new MqttError("the handshake failed", { code: MqttErrorCode.Handshake });
}
