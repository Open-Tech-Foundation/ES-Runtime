/**
 * The connection: one socket, one AMQP session, and the channels multiplexed
 * over it.
 *
 * The handshake is a short scripted exchange — the broker proposes a version,
 * we answer with a client identity; it proposes tuning values, we agree to
 * them; it opens the virtual host. Everything here is that script, plus the two
 * loops that follow: one reading frames and routing them, one sending
 * heartbeats.
 *
 * Three things this layer owns and no layer above it should have to:
 *
 * - **The frame boundary.** `FrameDecoder` accumulates whatever the socket
 *   delivered; this layer is what decides a frame is complete and what to do
 *   with it.
 * - **The channel number.** Channel 0 is the connection itself; frames for any
 *   other channel are routed to the handler registered for it. Which channels
 *   exist is this layer's bookkeeping, not the broker's.
 * - **Fatality.** A lost socket or a frame that is not AMQP leaves the stream
 *   at an unknown boundary, and every later frame would be misread. Such a
 *   failure is latched once and every caller — now and later — gets it, rather
 *   than each seeing a different symptom of the one dead connection.
 *
 * What it deliberately does not own: message assembly (a delivery is a method,
 * a header and some body frames, which is the channel's job) and reconnection
 * (`settings` are replayable, but deciding to replay is the pool's).
 */

import { unrefTimer } from "runtime:process";
import { Channel } from "./channel.js";
import { RabbitError, RabbitErrorCode } from "./errors.js";
import type { FieldTable } from "./protocol/fields.js";
import {
  DEFAULT_FRAME_MAX,
  encodeFrame,
  encodeProtocolHeader,
  FRAME_OVERHEAD,
  FrameDecoder,
  FrameType,
} from "./protocol/frame.js";
import {
  ClassId,
  type DecodedMethod,
  decodeMethod,
  encodeMethod,
  type MethodArg,
  methodName,
} from "./protocol/methods.js";
import {
  type BasicProperties,
  type ContentHeader,
  decodeContentHeader,
  encodeContentHeader,
} from "./protocol/properties.js";
import type { Transport } from "./transport.js";

/** The smallest frame the protocol allows either peer to insist on. */
const FRAME_MIN_SIZE = 4096;

/** The heartbeat RabbitMQ itself suggests when nothing else is asked for. */
export const DEFAULT_HEARTBEAT = 60;

/** How long a method waits for its answer, in ms, unless told otherwise. */
const DEFAULT_TIMEOUT = 30_000;

/** What the broker said about itself in `connection.start`. */
export interface ServerInfo {
  version: { major: number; minor: number };
  properties: FieldTable;
  mechanisms: string[];
  locales: string[];
}

export interface ConnectionSettings {
  /** The SASL PLAIN user. Defaults to RabbitMQ's `guest`. */
  username?: string;
  /** The password. Defaults to `guest`. */
  password?: string;
  /** The virtual host. Defaults to `/`. */
  vhost?: string;
  /**
   * The heartbeat to ask for, in seconds. Defaults to
   * {@link DEFAULT_HEARTBEAT}.
   *
   * `0` means "no proposal", which is the specification's rule and not the same
   * as "none": if the broker proposes an interval, the greater of the two is
   * used, so a broker that wants heartbeats gets them. Asking for `0` against a
   * broker that also proposes `0` is how heartbeats are turned off.
   */
  heartbeat?: number;
  /** The channels to ask for; `0` (the default) accepts the broker's limit. */
  channelMax?: number;
  /**
   * The largest frame to accept, in bytes; `0` (the default) accepts the
   * broker's. Never agreed below the protocol's 4096-octet minimum.
   */
  frameMax?: number;
  /** The message locale; RabbitMQ only offers `en_US`. */
  locale?: string;
  /** Extra client properties, merged over this client's own. */
  clientProperties?: FieldTable;
  /** How long the opening handshake may take, in ms. Defaults to `timeout`. */
  handshakeTimeout?: number;
  /** How long any one method waits for its answer, in ms. Defaults 30 000. */
  timeout?: number;
}

/**
 * What a channel has to supply to be driven by the connection.
 *
 * The four callbacks are the whole frame vocabulary a channel sees: a method,
 * the header that says how much body is coming, the body itself, and the news
 * that the channel is over — with the reason, or `null` when it was closed on
 * purpose.
 */
export interface ChannelHandler {
  onMethod(channel: number, method: DecodedMethod): void;
  onHeader(channel: number, header: ContentHeader): void;
  onBody(channel: number, bytes: Uint8Array): void;
  onClosed(channel: number, error: RabbitError | null): void;
}

interface Waiter {
  channel: number;
  names: readonly string[];
  resolve: (method: DecodedMethod) => void;
  reject: (error: RabbitError) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

/**
 * One AMQP connection: the handshake, the frame pump, the channels over it.
 *
 * The channel layer calls {@link send} and {@link sendContent} and takes a
 * {@link ChannelHandler} for the frames that come back; nothing above needs to
 * know a frame exists.
 */
export class Connection {
  readonly #transport: Transport;
  readonly #settings: ConnectionSettings;
  readonly #decoder = new FrameDecoder();

  #server: ServerInfo | null = null;
  #channelMax = 0;
  #frameMax: number = DEFAULT_FRAME_MAX;
  #heartbeat = 0;

  #handlers = new Map<number, ChannelHandler>();
  #waiters = new Set<Waiter>();
  #pump: Promise<void> | null = null;
  #fatal: RabbitError | null = null;
  #closing = false;
  #lastRead = 0;
  #heartbeatTimer: ReturnType<typeof setTimeout> | null = null;
  // Serialises writes. Frames on a channel may not overtake one another, and a
  // content message's method, header and body frames must go out as one unit —
  // a method that slipped between them is what the broker calls 505
  // UNEXPECTED_FRAME. Each logical send queues behind the last.
  #writing: Promise<void> = Promise.resolve();

  /** The broker went quiet about a resource (`connection.blocked`). */
  onBlocked: ((reason: string) => void) | null = null;
  /** And it is unblocked again (`connection.unblocked`). */
  onUnblocked: (() => void) | null = null;
  /** The connection ended, for any reason. `null` when closed on purpose. */
  onClosed: ((error: RabbitError | null) => void) | null = null;

  constructor(transport: Transport, settings: ConnectionSettings = {}) {
    this.#transport = transport;
    this.#settings = settings;
  }

  // -- what the broker agreed to --------------------------------------------

  /** The broker's identity, once the handshake has finished. */
  get server(): ServerInfo | null {
    return this.#server;
  }

  /** The channel limit in force; `0` means no limit beyond 65535. */
  get channelMax(): number {
    return this.#channelMax;
  }

  /** The largest frame either side will send, including header and end byte. */
  get frameMax(): number {
    return this.#frameMax;
  }

  /** The heartbeat interval in force, in seconds; `0` when disabled. */
  get heartbeat(): number {
    return this.#heartbeat;
  }

  /** Whether this connection is still fit to use. */
  get usable(): boolean {
    return this.#fatal === null && !this.#closing;
  }

  /** The failure that ended this connection, if one did. */
  get failure(): RabbitError | null {
    return this.#fatal;
  }

  // -- opening --------------------------------------------------------------

  /**
   * Runs the handshake. The transport is read from the moment this is called,
   * so the frames the broker sends first are not missed.
   */
  async open(): Promise<ServerInfo> {
    this.#lastRead = Date.now();
    this.#pump = this.#readLoop();
    const timeout = this.#settings.handshakeTimeout ?? this.#timeout();

    try {
      await this.#transport.write(encodeProtocolHeader());
      // A broker that will not speak this protocol answers with its own
      // protocol header rather than a frame, which the decoder reports as an
      // unknown frame type. That is a version refusal, so it is caught here and
      // said in those words.
      const start = await this.#expect(0, ["connection.start"], timeout);
      this.#checkVersion(start.args);
      this.#server = {
        version: { major: number(start.args.versionMajor), minor: number(start.args.versionMinor) },
        properties: table(start.args.serverProperties),
        mechanisms: String(start.args.mechanisms)
          .split(" ")
          .filter((one) => one !== ""),
        locales: String(start.args.locales)
          .split(" ")
          .filter((one) => one !== ""),
      };
      if (!this.#server.mechanisms.includes("PLAIN")) {
        throw new RabbitError(
          `the broker offers no PLAIN mechanism (it offers ${this.#server.mechanisms.join(", ") || "none"})`,
          { code: RabbitErrorCode.Handshake },
        );
      }

      await this.#send(0, "connection.start-ok", {
        clientProperties: clientProperties(this.#settings.clientProperties),
        mechanism: "PLAIN",
        response: `\u0000${this.#settings.username ?? "guest"}\u0000${this.#settings.password ?? "guest"}`,
        locale: this.#settings.locale ?? this.#server.locales[0] ?? "en_US",
      });

      // PLAIN authenticates in one shot, so a challenge cannot arrive: the
      // broker has nothing to challenge with.
      const tune = await this.#expect(0, ["connection.tune", "connection.secure"], timeout);
      if (tune.name === "connection.secure") {
        throw new RabbitError("the broker asked for a SASL challenge, which PLAIN never sends", {
          code: RabbitErrorCode.Handshake,
        });
      }
      this.#negotiate(tune.args);
      await this.#send(0, "connection.tune-ok", {
        channelMax: this.#channelMax,
        frameMax: this.#frameMax,
        heartbeat: this.#heartbeat,
      });
      await this.#send(0, "connection.open", { virtualHost: this.#settings.vhost ?? "/" });
      await this.#expect(0, ["connection.open-ok"], timeout);
    } catch (e) {
      // A refusal during the handshake is about credentials: a bad password
      // arrives as a `connection.close` in place of `connection.tune`, a vhost
      // the credentials cannot reach as one in place of `connection.open-ok`.
      const failure = this.#asHandshakeFailure(e);
      this.#fail(failure);
      throw failure;
    }

    this.#startHeartbeat();
    return this.#server;
  }

  #timeout(): number {
    return this.#settings.timeout ?? DEFAULT_TIMEOUT;
  }

  #checkVersion(args: Record<string, MethodArg>): void {
    const major = number(args.versionMajor);
    const minor = number(args.versionMinor);
    if (major !== 0 || minor !== 9) {
      throw new RabbitError(`the broker speaks AMQP ${major}.${minor}, not 0.9.1`, {
        code: RabbitErrorCode.Handshake,
      });
    }
  }

  /**
   * The tuning negotiation, whose rules are the specification's (§4.2.7.1): a
   * zero on either side means "no proposal", so the other value stands, and
   * otherwise the smaller of the two is agreed. Frame-max has a floor — neither
   * peer may insist on less than {@link FRAME_MIN_SIZE} — and a frame-max of
   * zero, which means "unlimited", is capped at this client's own default.
   */
  #negotiate(args: Record<string, MethodArg>): void {
    const brokerChannels = number(args.channelMax);
    const brokerFrames = number(args.frameMax);
    const brokerHeartbeat = number(args.heartbeat);

    this.#channelMax = agree(this.#settings.channelMax ?? 0, brokerChannels);
    this.#frameMax = Math.max(
      FRAME_MIN_SIZE,
      agree(this.#settings.frameMax ?? 0, brokerFrames) || DEFAULT_FRAME_MAX,
    );
    this.#heartbeat = agree(this.#settings.heartbeat ?? DEFAULT_HEARTBEAT, brokerHeartbeat);
  }

  /** A handshake failure the broker provoked is about credentials or vhost. */
  #asHandshakeFailure(e: unknown): RabbitError {
    if (
      e instanceof RabbitError &&
      e.code === RabbitErrorCode.Server &&
      e.replyCode !== null &&
      (e.replyCode === 403 || e.replyCode === 402 || e.replyCode === 530)
    ) {
      return new RabbitError(
        `the broker refused the connection: ${e.replyCode} ${e.replyText ?? ""}`.trimEnd(),
        {
          code: RabbitErrorCode.Auth,
          replyCode: e.replyCode,
          ...(e.replyText === null ? {} : { replyText: e.replyText }),
          cause: e,
        },
      );
    }
    return e instanceof RabbitError ? e : this.#wrap(e);
  }

  // -- channels -------------------------------------------------------------

  /**
   * Opens a channel and hands it to `handler`.
   *
   * The handler is registered *before* the method is sent, because the broker
   * may answer immediately and a frame arriving for a channel nobody is
   * registered for is dropped.
   */
  async openChannel(handler: ChannelHandler): Promise<number> {
    this.#requireUsable();
    const channel = this.#freeChannel();
    this.#handlers.set(channel, handler);
    try {
      await this.#send(channel, "channel.open", {});
      await this.#expect(channel, ["channel.open-ok"], this.#timeout());
      return channel;
    } catch (e) {
      this.#handlers.delete(channel);
      throw e;
    }
  }

  /** Closes a channel and waits for the broker to agree. */
  async closeChannel(channel: number): Promise<void> {
    if (!this.#handlers.has(channel) || this.#fatal !== null) {
      this.#forget(channel, null);
      return;
    }
    try {
      await this.#send(channel, "channel.close", {
        replyCode: 200,
        replyText: "Goodbye",
        classId: 0,
        methodId: 0,
      });
      await this.#expect(channel, ["channel.close-ok"], this.#timeout());
    } catch {
      // A channel the broker already closed, or a connection going down: the
      // caller asked for it to be gone, and it is.
    }
    this.#forget(channel, null);
  }

  /** Whether a channel is currently open. */
  hasChannel(channel: number): boolean {
    return this.#handlers.has(channel);
  }

  /**
   * Opens a channel and returns it as a {@link Channel} — the surface an
   * application works in. The channel is this connection's own handler for its
   * number, so the frames it receives are the ones this class routes to it.
   */
  async channel(options: { timeout?: number } = {}): Promise<Channel> {
    const channel = new Channel(this, options);
    const number = await this.openChannel(channel);
    channel.attach(number);
    return channel;
  }

  #freeChannel(): number {
    const limit = this.#channelMax === 0 ? 0xffff : this.#channelMax;
    for (let id = 1; id <= limit; id++) {
      if (!this.#handlers.has(id)) return id;
    }
    throw new RabbitError(`every one of the ${limit} channels is in use`, {
      code: RabbitErrorCode.Channel,
    });
  }

  // -- sending --------------------------------------------------------------

  /** Sends a method on a channel. The low-level surface the channel layer uses. */
  async send(channel: number, name: string, args: Record<string, MethodArg> = {}): Promise<void> {
    this.#requireUsable();
    await this.#send(channel, name, args);
  }

  /**
   * Sends a method that carries content: the method, a header describing the
   * body, and as many body frames as the frame-max requires.
   *
   * The body is split rather than refused when it is large — that is what the
   * negotiated frame-max is for — and each body frame is at most frame-max
   * minus the eight bytes of frame overhead. The three (or more) frames are one
   * atomic write, so a second publish on the channel cannot land between them.
   */
  async sendContent(
    channel: number,
    name: string,
    args: Record<string, MethodArg>,
    properties: BasicProperties,
    body: Uint8Array,
  ): Promise<void> {
    this.#requireUsable();
    const header = encodeContentHeader({
      classId: ClassId.basic,
      weight: 0,
      bodySize: BigInt(body.length),
      properties,
    });
    const chunk = this.#frameMax - FRAME_OVERHEAD;
    await this.#atomic(async () => {
      await this.#sendFrame(channel, name, args);
      await this.#writeRaw(encodeFrame({ type: FrameType.header, channel, payload: header }));
      for (let at = 0; at < body.length; at += chunk) {
        await this.#writeRaw(
          encodeFrame({ type: FrameType.body, channel, payload: body.subarray(at, at + chunk) }),
        );
      }
    });
  }

  async #send(channel: number, name: string, args: Record<string, MethodArg>): Promise<void> {
    await this.#atomic(() => this.#sendFrame(channel, name, args));
  }

  // One method frame. Deliberately not `#requireUsable`: the last frame a
  // connection sends is its own `connection.close`, which is sent *after*
  // closing has begun.
  async #sendFrame(channel: number, name: string, args: Record<string, MethodArg>): Promise<void> {
    if (this.#fatal !== null) throw this.#fatal;
    await this.#writeRaw(
      encodeFrame({ type: FrameType.method, channel, payload: encodeMethod(name, args) }),
    );
  }

  // One frame, on its own. A heartbeat cannot split a content message in two
  // any more than another publish can.
  async #write(bytes: Uint8Array): Promise<void> {
    await this.#atomic(() => this.#writeRaw(bytes));
  }

  async #writeRaw(bytes: Uint8Array): Promise<void> {
    try {
      await this.#transport.write(bytes);
    } catch (e) {
      throw this.#fail(this.#wrap(e));
    }
  }

  // Queues a unit of one or more frames so nothing else writes inside it. The
  // chain keeps running after a failure — the failure is the caller's to see,
  // and the next unit still has to go out or fail on its own.
  #atomic(unit: () => Promise<void>): Promise<void> {
    const next = this.#writing.then(unit, unit);
    this.#writing = next.then(
      () => {},
      () => {},
    );
    return next;
  }

  #requireUsable(): void {
    if (this.#fatal !== null) throw this.#fatal;
    if (this.#closing) {
      throw new RabbitError("the connection is closing", { code: RabbitErrorCode.Closed });
    }
  }

  // -- reading --------------------------------------------------------------

  async #readLoop(): Promise<void> {
    try {
      for (;;) {
        const chunk = await this.#transport.read();
        if (chunk === null) {
          throw new RabbitError("the broker closed the connection", {
            code: RabbitErrorCode.Connection,
          });
        }
        this.#lastRead = Date.now();
        this.#decoder.feed(chunk);
        for (let frame = this.#decoder.next(); frame !== null; frame = this.#decoder.next()) {
          this.#route(frame.channel, frame.type, frame.payload);
        }
      }
    } catch (e) {
      if (this.#closing && this.#fatal === null) return;
      this.#fail(e instanceof RabbitError ? e : this.#wrap(e));
    }
  }

  #route(channel: number, type: number, payload: Uint8Array): void {
    if (type === FrameType.heartbeat) return;
    if (type === FrameType.method) {
      this.#deliverMethod(channel, decodeMethod(payload));
      return;
    }
    const handler = this.#handlers.get(channel);
    if (handler === undefined) return;
    if (type === FrameType.header) handler.onHeader(channel, decodeContentHeader(payload));
    else handler.onBody(channel, payload);
  }

  #deliverMethod(channel: number, method: DecodedMethod): void {
    const waiter = this.#takeWaiter(channel, method.name);
    if (waiter !== null) {
      waiter(method);
      return;
    }
    if (channel === 0) {
      this.#connectionMethod(method);
      return;
    }
    // A channel the broker closed: soft, and the channel's handler is the only
    // one that can decide what its outstanding work should do.
    if (method.name === "channel.close") {
      const error = serverError(method);
      void this.#send(channel, "channel.close-ok", {}).catch(() => {});
      this.#forget(channel, error);
      return;
    }
    this.#handlers.get(channel)?.onMethod(channel, method);
  }

  #connectionMethod(method: DecodedMethod): void {
    switch (method.name) {
      case "connection.close": {
        const error = serverError(method);
        if (!this.#closing) void this.#send(0, "connection.close-ok", {}).catch(() => {});
        this.#fail(error);
        return;
      }
      case "connection.close-ok":
        return;
      case "connection.blocked":
        this.onBlocked?.(String(method.args.reason));
        return;
      case "connection.unblocked":
        this.onUnblocked?.();
        return;
      default:
        // Anything else on channel 0 that nobody is waiting for is a method
        // this client does not know, which is not a reason to break.
        return;
    }
  }

  /** The first waiter for `channel` that lists `name`, removed and answered. */
  #takeWaiter(channel: number, name: string): ((method: DecodedMethod) => void) | null {
    for (const waiter of this.#waiters) {
      if (waiter.channel !== channel) continue;
      if (!waiter.names.includes(name)) continue;
      this.#settle(waiter);
      return waiter.resolve;
    }
    return null;
  }

  async #expect(
    channel: number,
    names: readonly string[],
    timeout: number,
  ): Promise<DecodedMethod> {
    // As with `#send`: a close still waits for its `…-ok`, so this must not
    // refuse merely because closing has begun.
    if (this.#fatal !== null) throw this.#fatal;
    return new Promise((resolve, reject) => {
      const waiter: Waiter = { channel, names, resolve, reject, timer: null };
      if (timeout > 0) {
        waiter.timer = setTimeout(() => {
          this.#waiters.delete(waiter);
          reject(
            new RabbitError(`no ${names.join(" or ")} from the broker within ${timeout}ms`, {
              code: RabbitErrorCode.Timeout,
            }),
          );
        }, timeout);
      }
      this.#waiters.add(waiter);
    });
  }

  #settle(waiter: Waiter): void {
    if (waiter.timer !== null) clearTimeout(waiter.timer);
    this.#waiters.delete(waiter);
  }

  // -- heartbeats -----------------------------------------------------------

  /**
   * Sends a heartbeat every half-interval, and treats two missed intervals as a
   * lost connection. The timer does not hold the process open: a program whose
   * only remaining work is a broker's pulse should be able to exit.
   */
  #startHeartbeat(): void {
    if (this.#heartbeat <= 0) return;
    const every = Math.max(1000, (this.#heartbeat * 1000) / 2);
    const tick = () => {
      if (this.#fatal !== null || this.#closing) return;
      if (Date.now() - this.#lastRead > this.#heartbeat * 2000) {
        this.#fail(
          new RabbitError(
            `the broker sent nothing for ${this.#heartbeat * 2}s — treating the connection as lost`,
            { code: RabbitErrorCode.Timeout },
          ),
        );
        return;
      }
      void this.#write(
        encodeFrame({ type: FrameType.heartbeat, channel: 0, payload: new Uint8Array(0) }),
      ).catch(() => {});
      this.#heartbeatTimer = setTimeout(tick, every);
      unrefTimer(this.#heartbeatTimer as unknown as number);
    };
    this.#heartbeatTimer = setTimeout(tick, every);
    unrefTimer(this.#heartbeatTimer as unknown as number);
  }

  // -- closing --------------------------------------------------------------

  /** Closes the connection the way the protocol wants: `connection.close`. */
  async close(): Promise<void> {
    if (this.#fatal !== null) {
      await this.#teardown();
      return;
    }
    if (this.#closing) return;
    this.#closing = true;
    try {
      await this.#send(0, "connection.close", {
        replyCode: 200,
        replyText: "Goodbye",
        classId: 0,
        methodId: 0,
      });
      await this.#expect(0, ["connection.close-ok"], this.#timeout());
    } catch {
      // The broker may have closed first, or not answer at all. Either way the
      // socket goes next, and that is what actually ends the session.
    }
    await this.#teardown();
  }

  #forget(channel: number, error: RabbitError | null): void {
    const handler = this.#handlers.get(channel);
    if (handler === undefined) return;
    this.#handlers.delete(channel);
    for (const waiter of [...this.#waiters]) {
      if (waiter.channel !== channel) continue;
      this.#settle(waiter);
      waiter.reject(
        error ??
          new RabbitError(`channel ${channel} was closed`, { code: RabbitErrorCode.Channel }),
      );
    }
    handler.onClosed(channel, error);
  }

  #fail(error: RabbitError): RabbitError {
    if (this.#fatal === null) {
      this.#fatal = error;
      this.#stopHeartbeat();
      for (const waiter of [...this.#waiters]) {
        this.#settle(waiter);
        waiter.reject(error);
      }
      for (const channel of [...this.#handlers.keys()]) this.#forget(channel, error);
      this.onClosed?.(error);
      void this.#teardown();
    }
    return this.#fatal;
  }

  async #teardown(): Promise<void> {
    this.#stopHeartbeat();
    await this.#transport.close().catch(() => {});
    if (this.#pump !== null) await this.#pump.catch(() => {});
  }

  #stopHeartbeat(): void {
    if (this.#heartbeatTimer !== null) {
      clearTimeout(this.#heartbeatTimer);
      this.#heartbeatTimer = null;
    }
  }

  #wrap(cause: unknown): RabbitError {
    const detail = cause instanceof Error ? cause.message : String(cause);
    return new RabbitError(`the connection to the broker was lost: ${detail}`, {
      code: RabbitErrorCode.Connection,
      cause,
    });
  }
}

// -- helpers -----------------------------------------------------------------

/** The specification's tuning rule: zero proposes nothing, otherwise take less. */
function agree(ours: number, theirs: number): number {
  return ours === 0 || theirs === 0 ? Math.max(ours, theirs) : Math.min(ours, theirs);
}

function number(value: MethodArg | undefined): number {
  if (typeof value === "number") return value;
  if (typeof value === "bigint") return Number(value);
  throw new RabbitError(`expected a number, found ${typeof value}`, {
    code: RabbitErrorCode.Protocol,
  });
}

function table(value: MethodArg | undefined): FieldTable {
  if (value instanceof Map) return value;
  throw new RabbitError("expected a field table", { code: RabbitErrorCode.Protocol });
}

/** A refusing `channel.close` or `connection.close` as an error. */
function serverError(method: DecodedMethod): RabbitError {
  const replyCode = number(method.args.replyCode);
  const replyText = String(method.args.replyText ?? "");
  const classId = number(method.args.classId);
  const methodId = number(method.args.methodId);
  const failing = classId === 0 && methodId === 0 ? null : methodName(classId, methodId);
  return new RabbitError(
    `${failing === null ? "the broker" : failing} refused: ${replyCode} ${replyText}`.trimEnd(),
    {
      code: RabbitErrorCode.Server,
      replyCode,
      replyText,
      ...(classId === 0 && methodId === 0 ? {} : { failing: { classId, methodId } }),
    },
  );
}

/**
 * What this client tells the broker about itself.
 *
 * The capabilities are the load-bearing part: they are how the broker learns it
 * may send `basic.nack`, that it should tell us when a consumer is cancelled,
 * that it may report itself blocked, and that publisher confirms are
 * understood. A client that stays silent here is treated as one that speaks
 * none of them.
 */
function clientProperties(extra?: FieldTable): FieldTable {
  const capabilities: FieldTable = new Map([
    ["publisher_confirms", { type: "boolean", value: true }],
    ["consumer_cancel_notify", { type: "boolean", value: true }],
    ["connection.blocked", { type: "boolean", value: true }],
    ["basic.nack", { type: "boolean", value: true }],
    ["authentication_failure_close", { type: "boolean", value: true }],
  ]);
  const properties: FieldTable = new Map([
    ["product", { type: "string", value: "esrun-rabbitmq" }],
    ["platform", { type: "string", value: "esrun" }],
    ["information", { type: "string", value: "https://esrun.opentechf.org/docs/guides/rabbitmq" }],
    ["capabilities", { type: "table", value: capabilities }],
  ]);
  if (extra !== undefined) {
    for (const [key, value] of extra) properties.set(key, value);
  }
  return properties;
}
