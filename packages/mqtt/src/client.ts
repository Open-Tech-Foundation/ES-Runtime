/**
 * The client: publish, subscribe, and request/reply over a session that
 * outlives its connections.
 *
 * Everything the connection answers on the protocol's behalf — PUBREL and
 * PUBCOMP, PINGREQ — has already happened by the time a {@link Message}
 * arrives here. What is left is what an application asked for:
 *
 * - **Publish** at QoS 0, 1 or 2, where the QoS 1 and 2 calls resolve only when
 *   the broker has acknowledged (or refuse, with the broker's reason code, when
 *   it would not take the message).
 * - **Subscribe**, which registers a topic filter and hands back an async
 *   iterable of {@link Message}; the connection's messages are routed to it by
 *   the filter, and the two wildcards are matched here, not guessed at. An
 *   inbound QoS 1 or 2 message is acknowledged when a subscription hands it to
 *   the application — "having accepted ownership" (MQTT 5.0, 4.3.2) — so the
 *   advertised Receive Maximum is what bounds how far ahead the broker sends.
 * - **Request/reply**, which is the 5.0 convention and nothing more: publish
 *   with a response topic and correlation data, and read the first reply that
 *   carries the same correlation data back.
 *
 * A lost connection is reconnected (DECISIONS D178): with backoff, resuming
 * the session where one can survive. A resumed session resends what was in
 * flight with its original identifiers (4.4); a fresh one rejects it — never
 * republished as a new message — and has every subscription sent again.
 */

import { unrefTimer } from "runtime:process";
import {
  Connection,
  type ConnectionHandlers,
  type ConnectionSettings,
  type Dialer,
  type InboundSession,
  openWithDial,
} from "./connection.js";
import { MqttError, MqttErrorCode } from "./errors.js";
import {
  type AckPacket,
  type Packet,
  PacketType,
  type PublishPacket,
  type SubackPacket,
  type SubscribeEntry,
  type UnsubackPacket,
} from "./protocol/packets.js";
import type { Properties } from "./protocol/properties.js";
import { isErrorReason, reasonName } from "./protocol/reason.js";
import { topicMatches, validateTopicName } from "./protocol/topic.js";
import { netTransport, type Transport } from "./transport.js";

const DEFAULT_TIMEOUT = 30_000;

const ENCODER = new TextEncoder();

/** A message delivered to a subscriber. */
export class Message {
  /** The topic it was published to. */
  readonly topic: string;
  /** The payload, as bytes. */
  readonly payload: Uint8Array;
  /** The QoS it was delivered at. */
  readonly qos: 0 | 1 | 2;
  /** Whether the message was a retained one. */
  readonly retain: boolean;
  /** Whether the broker marked it a retransmission. */
  readonly dup: boolean;
  /** The 5.0 properties, or `{}` on a 3.1.1 connection. */
  readonly properties: Properties;

  constructor(packet: PublishPacket) {
    this.topic = packet.topic;
    this.payload = packet.payload;
    this.qos = packet.qos;
    this.retain = packet.retain;
    this.dup = packet.dup;
    this.properties = packet.properties ?? {};
  }

  /** The payload decoded as text. */
  text(encoding: string = "utf-8"): string {
    return new TextDecoder(encoding).decode(this.payload);
  }

  /** The payload parsed as JSON. */
  json(): unknown {
    return JSON.parse(this.text());
  }

  /** The correlation data, when the sender set it (5.0). */
  get correlationData(): Uint8Array | null {
    return this.properties.correlationData ?? null;
  }

  /** The response topic, when the sender set one (5.0). */
  get responseTopic(): string | null {
    return this.properties.responseTopic ?? null;
  }

  /** The user properties, in order (5.0). */
  get userProperties(): Array<[string, string]> {
    return this.properties.userProperties ?? [];
  }
}

/** What to publish with. */
export interface PublishOptions {
  qos?: 0 | 1 | 2;
  retain?: boolean;
  /** Seconds before the broker drops the message (5.0). */
  messageExpiryInterval?: number;
  contentType?: string;
  /** The topic a reply should go to (5.0, used by {@link MqttClient.request}). */
  responseTopic?: string;
  correlationData?: Uint8Array;
  userProperties?: Array<[string, string]>;
  /** Any further 5.0 properties. */
  properties?: Properties;
  /**
   * How long the publish may take, in ms — waiting out a reconnect, room under
   * the broker's Receive Maximum, and the acknowledgement. Defaults 30 000.
   */
  timeout?: number;
}

/** What to subscribe with. */
export interface SubscribeOptions {
  qos?: 0 | 1 | 2;
  /** Do not send this client its own publishes back (5.0). */
  noLocal?: boolean;
  retainAsPublished?: boolean;
  retainHandling?: 0 | 1 | 2;
  /** A subscription identifier the broker echoes on matching messages (5.0). */
  subscriptionIdentifier?: number;
  /** Any further 5.0 properties. */
  properties?: Properties;
  /**
   * The most QoS 0 messages held unread; past it the oldest QoS 0 message is
   * dropped and counted in {@link Subscription.dropped}. QoS 1 and 2 are never
   * dropped — the Receive Maximum bounds them. Default 10 000.
   */
  maxBuffered?: number;
  /** How long to wait for the SUBACK (and any reconnect), in ms. Defaults 30 000. */
  timeout?: number;
}

/** Reconnect timing: exponential backoff with jitter between these bounds. */
export interface ReconnectOptions {
  /** The first retry's delay, in ms. Default 500. */
  initialDelay?: number;
  /** The longest delay between retries, in ms. Default 30 000. */
  maxDelay?: number;
}

/** Any extra connection settings, plus a default operation timeout. */
export interface MqttClientOptions extends ConnectionSettings {
  /** The default timeout for an operation, in ms. Defaults 30 000. */
  timeout?: number;
  /**
   * Receives every message no subscription on this client matches. A
   * persistent session (`cleanStart: false`) delivers what queued while the
   * client was away straight after the CONNACK — before the application has
   * subscribed again — and those land here instead of being dropped. Set it
   * here, not later, to see them.
   */
  onUnmatchedMessage?: (message: Message) => void;
  /**
   * Reconnecting after a lost connection (DECISIONS D178). On by default;
   * `false` ends the client with the first failure instead.
   */
  reconnect?: false | ReconnectOptions;
  /** How a connection reaches the broker's socket; a test scripts one. */
  dial?: Dialer;
}

const DEFAULT_MAX_BUFFERED = 10_000;
const DEFAULT_INITIAL_DELAY = 500;
const DEFAULT_MAX_DELAY = 30_000;

/**
 * The acknowledgement owed for one delivery. A message matching several
 * subscriptions is acknowledged once every one of them has taken it — or ended
 * without reading it.
 */
class Ticket {
  #remaining: number;
  readonly #accept: (() => void) | null;

  constructor(remaining: number, accept: (() => void) | null) {
    this.#remaining = remaining;
    this.#accept = accept;
    if (remaining === 0) accept?.();
  }

  take(): void {
    if (this.#remaining === 0) return;
    this.#remaining--;
    if (this.#remaining === 0) this.#accept?.();
  }
}

interface Entry {
  message: Message;
  ticket: Ticket;
}

/** A live subscription: an async iterable of messages, until unsubscribed. */
export class Subscription implements AsyncIterable<Message> {
  readonly filter: string;
  #client: MqttClient;
  #queue: Entry[] = [];
  #waiters: Array<(result: IteratorResult<Message>) => void> = [];
  #closed = false;
  #grantedQos: 0 | 1 | 2 = 0;
  #granted = false;
  #dropped = 0;
  readonly #maxBuffered: number;
  readonly #options: SubscribeOptions;

  /** @internal */
  constructor(client: MqttClient, filter: string, options: SubscribeOptions) {
    this.#client = client;
    this.filter = filter;
    this.#options = options;
    this.#maxBuffered = options.maxBuffered ?? DEFAULT_MAX_BUFFERED;
  }

  /** The QoS the broker granted for this filter. */
  get grantedQos(): 0 | 1 | 2 {
    return this.#grantedQos;
  }

  /** Whether the subscription is over. */
  get closed(): boolean {
    return this.#closed;
  }

  /** QoS 0 messages dropped because `maxBuffered` were already waiting unread. */
  get dropped(): number {
    return this.#dropped;
  }

  /** @internal The options it was made with, for a resubscribe. */
  get options(): SubscribeOptions {
    return this.#options;
  }

  /** @internal Whether the broker has granted it (a pending one resends itself). */
  get granted(): boolean {
    return this.#granted;
  }

  /** @internal */
  setGrantedQos(qos: 0 | 1 | 2): void {
    this.#grantedQos = qos;
    this.#granted = true;
  }

  /** @internal */
  offer(message: Message, ticket: Ticket): void {
    if (this.#closed) {
      ticket.take();
      return;
    }
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) {
      ticket.take();
      waiter({ value: message, done: false });
      return;
    }
    if (message.qos === 0 && this.#queue.length >= this.#maxBuffered) {
      // QoS 0 is at-most-once and cannot be flow-controlled; past the bound
      // the oldest one goes, so memory stays bounded under a flood.
      const oldest = this.#queue.findIndex((entry) => entry.message.qos === 0);
      if (oldest >= 0) this.#queue.splice(oldest, 1);
      this.#dropped++;
      if (oldest < 0) return;
    }
    this.#queue.push({ message, ticket });
  }

  /**
   * @internal After a resumed session: the broker resends every QoS 1 and 2
   * message it had not had acknowledged, so the copies buffered here — whose
   * acknowledgement belonged to the dead connection — are dropped.
   */
  discardReliable(): void {
    this.#queue = this.#queue.filter((entry) => entry.message.qos === 0);
  }

  /** @internal */
  finish(): void {
    if (this.#closed) return;
    this.#closed = true;
    // Unread messages are acknowledged as they are discarded: MQTT has no way
    // to hand one back, and an unacknowledged one would hold a slot of the
    // broker's send quota for good.
    for (const entry of this.#queue.splice(0)) entry.ticket.take();
    for (const waiter of this.#waiters.splice(0)) waiter({ value: undefined, done: true });
  }

  /** Whether `topic` matches this subscription's filter. */
  matches(topic: string): boolean {
    return topicMatches(this.filter, topic);
  }

  /** The next message, or `done` once the subscription ends. */
  next(): Promise<IteratorResult<Message>> {
    const entry = this.#queue.shift();
    if (entry !== undefined) {
      entry.ticket.take();
      return Promise.resolve({ value: entry.message, done: false });
    }
    if (this.#closed) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  /** The next message, rejecting if none arrives within `timeout` ms. */
  nextMessage(timeout: number = DEFAULT_TIMEOUT): Promise<Message> {
    const entry = this.#queue.shift();
    if (entry !== undefined) {
      entry.ticket.take();
      return Promise.resolve(entry.message);
    }
    if (this.#closed) {
      return Promise.reject(
        new MqttError("the subscription is closed", { code: MqttErrorCode.Closed }),
      );
    }
    return new Promise<Message>((resolve, reject) => {
      let timer: ReturnType<typeof setTimeout>;
      const waiter = (result: IteratorResult<Message>) => {
        clearTimeout(timer);
        if (result.done) {
          reject(new MqttError("the subscription is closed", { code: MqttErrorCode.Closed }));
        } else {
          resolve(result.value);
        }
      };
      timer = setTimeout(() => {
        // Remove the waiter only if it is still queued. A delivery may have taken
        // it already, and leaving it behind would let the *next* message resolve
        // this abandoned waiter — which is to say, lose that message.
        if (this.#removeWaiter(waiter)) {
          reject(new MqttError("no message arrived in time", { code: MqttErrorCode.Timeout }));
        }
      }, timeout);
      unrefTimer(timer as unknown as number);
      this.#waiters.push(waiter);
    });
  }

  #removeWaiter(waiter: (result: IteratorResult<Message>) => void): boolean {
    const at = this.#waiters.indexOf(waiter);
    if (at < 0) return false;
    this.#waiters.splice(at, 1);
    return true;
  }

  /**
   * Ends this subscription. The broker holds one subscription per filter for a
   * client, so the UNSUBSCRIBE is sent only when this was the last local
   * subscription to its filter; any other keeps receiving.
   */
  async unsubscribe(): Promise<void> {
    if (this.#closed) return;
    await this.#client.release(this);
    this.finish();
  }

  [Symbol.asyncIterator](): AsyncIterator<Message> {
    return this;
  }
}

/** Where one dial's messages wait until its connection is the client's. */
interface Slot {
  connection: Connection | null;
  early: Array<[PublishPacket, (() => void) | null]>;
}

type Waiter<T> = {
  resolve: (value: T) => void;
  reject: (error: MqttError) => void;
  timer: ReturnType<typeof setTimeout>;
};

/** An outbound QoS 1 or 2 publish not yet finished, for a resumed session to resend. */
interface Outbound {
  packet: PublishPacket;
  /** `publish` until a successful PUBREC; then `pubrel` (QoS 2). */
  stage: "publish" | "pubrel";
}

/** A broker DISCONNECT after which reconnecting is the wrong thing to do. */
const FINAL_DISCONNECTS: ReadonlySet<number> = new Set([
  0x87, // Not authorized
  0x8a, // Banned
  0x8e, // Session taken over — another client holds this id now
  0x9c, // Use another server
  0x9d, // Server moved
]);

/**
 * A refusal no retry will change: the broker refusing the CONNECT, and a
 * socket that `runtime:net` would not open for a reason of its own — a TLS
 * failure such as an untrusted certificate (`ERR_TLS`), or a host the
 * `--allow-net` grant does not name (`ERR_CAPABILITY_DENIED`). Retrying those
 * only delays the same answer until the timeout.
 */
function refusal(error: unknown): boolean {
  if (!(error instanceof MqttError)) return false;
  if (
    error.code === MqttErrorCode.Auth ||
    error.code === MqttErrorCode.Refused ||
    error.code === MqttErrorCode.Handshake ||
    error.code === MqttErrorCode.Unsupported ||
    error.code === MqttErrorCode.Protocol
  ) {
    return true;
  }
  const cause = (error as { cause?: { code?: unknown } }).cause?.code;
  return cause === "ERR_TLS" || cause === "ERR_CAPABILITY_DENIED";
}

/** What a SUBSCRIBE or UNSUBSCRIBE in flight gets when its connection is lost. */
class Interrupted extends MqttError {
  constructor() {
    super("the connection was lost before the broker answered", {
      code: MqttErrorCode.Connection,
    });
  }
}

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

export class MqttClient {
  readonly #url: string | null;
  readonly #options: MqttClientOptions;
  readonly #dial: Dialer;
  readonly #reconnect: Required<ReconnectOptions> | null;
  readonly #inbound: InboundSession = { awaitingRelease: new Set() };

  #connection: Connection | null = null;
  #level: 4 | 5 | null = null;
  #clientId: string | undefined;
  #ready = new Signal<Connection>();
  #attempt = 0;
  #timer: ReturnType<typeof setTimeout> | null = null;
  // Attempts whose socket opened and whose handshake then failed. A proxy in
  // front of the broker (Docker's port proxy, a load balancer) can accept such
  // a socket while the broker is down and deliver its CONNECT once the broker
  // is back — after a later attempt has already connected. The broker then
  // hands the session to that ghost and evicts the live connection with
  // "Session taken over". Seen against Mosquitto behind `docker run -p`.
  #ghosts = 0;

  #subscriptions: Subscription[] = [];
  #outbound = new Map<number, Outbound>();
  // QoS 1 and 2 publishes holding a slot of the broker's Receive Maximum, and
  // those waiting for one.
  #inFlight = 0;
  #slotWaiters: Array<() => void> = [];
  #ackWaiters = new Map<number, Waiter<AckPacket>>();
  #subackWaiters = new Map<number, Waiter<SubackPacket>>();
  #unsubackWaiters = new Map<number, Waiter<UnsubackPacket>>();
  #closed = false;
  #failure: MqttError | null = null;

  /** Messages no subscription matches. See {@link MqttClientOptions.onUnmatchedMessage}. */
  onUnmatchedMessage: ((message: Message) => void) | null = null;
  /** The connection was lost; reconnecting has begun. */
  onDisconnect: ((error: MqttError) => void) | null = null;
  /** A connection is open again, and the session resumed or rebuilt. */
  onReconnect: (() => void) | null = null;
  /** A reconnect attempt failed; it will be retried. */
  onError: ((error: MqttError) => void) | null = null;

  private constructor(url: string | null, options: MqttClientOptions) {
    this.#url = url;
    this.#options = options;
    this.#dial = options.dial ?? netTransport;
    this.#clientId = options.clientId;
    this.#reconnect =
      url === null || options.reconnect === false
        ? null
        : {
            initialDelay: options.reconnect?.initialDelay ?? DEFAULT_INITIAL_DELAY,
            maxDelay: options.reconnect?.maxDelay ?? DEFAULT_MAX_DELAY,
          };
    this.onUnmatchedMessage = options.onUnmatchedMessage ?? null;
  }

  /**
   * Connects to `url` and completes the handshake. A transient failure — a
   * refused or lost socket, a timeout — is retried with backoff until
   * `options.timeout`; a refused connection (credentials, identifier, version)
   * rejects at once. Once open, a lost connection is reconnected unless
   * `reconnect: false`.
   */
  static async connect(url: string, options: MqttClientOptions = {}): Promise<MqttClient> {
    const client = new MqttClient(url, options);
    await client.#first();
    return client;
  }

  /**
   * Connects over an already-established transport.
   *
   * The seam a test stands on: the client's routing, acknowledgement handling
   * and request/reply run the same whether the bytes come from a socket or a
   * scripted broker in memory. With no URL to dial again, such a client does
   * not reconnect.
   */
  static async over(
    transport: Transport,
    options: MqttClientOptions = {},
    level: 4 | 5 = 5,
  ): Promise<MqttClient> {
    const client = new MqttClient(null, options);
    const slot: Slot = { connection: null, early: [] };
    const connection = await Connection.over(
      transport,
      options,
      client.#handlers(slot),
      level,
      client.#inbound,
    );
    client.#adopt(connection, slot, true);
    return client;
  }

  /** The negotiated protocol level: 5 for MQTT 5.0, 4 for 3.1.1. */
  get protocolLevel(): 4 | 5 {
    return this.#connection?.protocolLevel ?? this.#level ?? 5;
  }

  /** Whether the broker kept a session for this client, on the latest connection. */
  get sessionPresent(): boolean {
    return this.#connection?.sessionPresent ?? false;
  }

  /** Whether a connection is open right now. */
  get connected(): boolean {
    return this.#connection !== null;
  }

  /** Whether the client has ended — closed, or failed for good. */
  get closed(): boolean {
    return this.#closed;
  }

  /** What ended the client for good, or `null` while it stands (reconnecting included). */
  get failure(): MqttError | null {
    return this.#failure;
  }

  /** Sends a PINGREQ and resolves when the PINGRESP arrives. */
  async ping(timeout?: number): Promise<void> {
    const connection = await this.#live(Date.now() + (timeout ?? this.#timeout()));
    return connection.ping(timeout);
  }

  /**
   * Publishes a message, resolving once the broker has acknowledged it (QoS 1
   * and 2) or once it is written (QoS 0). While disconnected it waits for the
   * reconnect, within its timeout. A QoS 1 or 2 publish in flight when the
   * connection drops is resent if the session is resumed and rejected if it is
   * not — it is never republished as a new message.
   */
  async publish(
    topic: string,
    payload: string | Uint8Array,
    options: PublishOptions = {},
  ): Promise<void> {
    this.#assertOpen();
    validateTopicName(topic);
    const qos = options.qos ?? 0;
    const bytes = typeof payload === "string" ? ENCODER.encode(payload) : payload;
    const properties = this.#publishProperties(options);
    const retain = options.retain ?? false;
    const timeout = options.timeout ?? this.#timeout();
    const deadline = Date.now() + timeout;

    const connection = await this.#live(deadline);
    this.#checkPublishAllowed(qos, retain);
    const base = {
      type: PacketType.Publish,
      topic,
      payload: bytes,
      retain,
      dup: false,
      ...(properties === undefined ? {} : { properties }),
    } as const;

    if (qos === 0) {
      await connection.send({ ...base, qos: 0 });
      return;
    }

    await this.#acquireSlot(Math.max(1, deadline - Date.now()));
    let ack: AckPacket;
    let packetId = 0;
    try {
      packetId = this.#allocatePacketId();
      const packet: PublishPacket = { ...base, qos, packetId };
      // QoS 1 waits for PUBACK; QoS 2 waits for PUBCOMP, the connection having
      // answered the PUBREC in between.
      const waiter = this.#waitFor(this.#ackWaiters, packetId, Math.max(1, deadline - Date.now()));
      this.#outbound.set(packetId, { packet, stage: "publish" });
      ack = await this.#awaitAck(
        connection,
        connection.send(packet),
        waiter,
        // A write that never left — an unencodable packet, or one the connection
        // refused — cannot leave its timer to reject alone.
        () => this.#forget(this.#ackWaiters, packetId),
        true,
      );
    } finally {
      this.#outbound.delete(packetId);
      this.#releaseSlot();
    }
    if (isErrorReason(ack.reasonCode)) {
      throw new MqttError(
        `the broker refused the publish: ${reasonName(ack.reasonCode) ?? `reason ${ack.reasonCode}`}`,
        {
          code: MqttErrorCode.Publish,
          reasonCode: ack.reasonCode,
          ...(reasonName(ack.reasonCode) === null
            ? {}
            : { reasonName: reasonName(ack.reasonCode)! }),
        },
      );
    }
  }

  /**
   * Subscribes to a topic filter and returns the messages it matches. It is
   * re-sent after a reconnect whose session the broker did not keep.
   */
  async subscribe(filter: string, options: SubscribeOptions = {}): Promise<Subscription> {
    this.#assertOpen();
    const deadline = Date.now() + (options.timeout ?? this.#timeout());
    const subscription = new Subscription(this, filter, options);
    // Register before the SUBACK, so a message that arrives with the grant is
    // routed rather than dropped.
    this.#subscriptions.push(subscription);
    try {
      for (;;) {
        const connection = await this.#live(deadline);
        this.#checkSubscribeAllowed(filter, options);
        try {
          const ack = await this.#subscribeOn(connection, filter, options, deadline);
          const reason = ack.reasons[0];
          if (reason === undefined || isErrorReason(reason)) {
            throw new MqttError(
              `the broker refused the subscription to ${JSON.stringify(filter)}: ${reasonName(reason ?? 0) ?? `reason ${reason}`}`,
              {
                code: MqttErrorCode.Subscription,
                ...(reason === undefined ? {} : { reasonCode: reason }),
                ...(reason === undefined || reasonName(reason) === null
                  ? {}
                  : { reasonName: reasonName(reason)! }),
              },
            );
          }
          subscription.setGrantedQos(reason as 0 | 1 | 2);
          return subscription;
        } catch (e) {
          // Lost before the SUBACK: send it again on the next connection.
          if (e instanceof Interrupted) continue;
          throw e;
        }
      }
    } catch (e) {
      this.#dropSubscription(subscription);
      throw e;
    }
  }

  /** Unsubscribes from one or more topic filters. */
  async unsubscribe(filters: string | string[], timeout?: number): Promise<void> {
    this.#assertOpen();
    const list = Array.isArray(filters) ? filters : [filters];
    const deadline = Date.now() + (timeout ?? this.#timeout());
    for (;;) {
      const connection = await this.#live(deadline);
      const packetId = this.#allocatePacketId();
      const waiter = this.#waitFor(
        this.#unsubackWaiters,
        packetId,
        Math.max(1, deadline - Date.now()),
      );
      try {
        await this.#awaitAck(
          connection,
          connection.send({ type: PacketType.Unsubscribe, packetId, filters: list }),
          waiter,
          () => this.#forget(this.#unsubackWaiters, packetId),
          false,
        );
        break;
      } catch (e) {
        if (e instanceof Interrupted) continue;
        throw e;
      }
    }
    for (const filter of list) this.#removeSubscriptions(filter);
  }

  /**
   * Publishes to `topic` and returns the first reply.
   *
   * This is the MQTT 5.0 request/response convention and needs 5.0: the request
   * carries a response topic and correlation data, and the reply to that topic
   * is matched on the correlation data. A 3.1.1 connection cannot carry either,
   * so it is refused rather than silently turned into a fire-and-forget publish.
   */
  async request(
    topic: string,
    payload: string | Uint8Array,
    options: PublishOptions = {},
  ): Promise<Message> {
    if (this.protocolLevel !== 5) {
      throw new MqttError("request/reply needs MQTT 5.0", { code: MqttErrorCode.Unsupported });
    }
    // Not under `$`: that space is the broker's own, and the specification says
    // a broker SHOULD stop clients exchanging messages on it (4.7.2). The token
    // is from the CSPRNG, since another client that guessed the topic could
    // answer in the responder's place.
    const responseTopic = options.responseTopic ?? `esrun/reply/${randomToken()}`;
    const correlationData = options.correlationData ?? randomBytes(16);
    const subscription = await this.subscribe(responseTopic, { qos: 0 });
    try {
      await this.publish(topic, payload, { ...options, responseTopic, correlationData });
      const timeout = options.timeout ?? this.#timeout();
      for (;;) {
        const reply = await subscription.nextMessage(timeout);
        if (matches(reply.correlationData, correlationData)) return reply;
      }
    } finally {
      await subscription.unsubscribe().catch(() => {});
    }
  }

  /** Closes the connection, ending every subscription, and stops reconnecting. */
  async close(options?: { reasonCode?: number; properties?: Properties }): Promise<void> {
    if (this.#closed) return;
    const connection = this.#connection;
    this.#end(new MqttError("the client was closed", { code: MqttErrorCode.Closed }), false);
    await connection?.close(options ?? {});
  }

  /** @internal Ends one local subscription; unsubscribes its filter if it was the last. */
  async release(subscription: Subscription): Promise<void> {
    const others = this.#subscriptions.some(
      (one) => one !== subscription && one.filter === subscription.filter,
    );
    if (others || this.#closed) {
      this.#dropSubscription(subscription);
      return;
    }
    await this.unsubscribe(subscription.filter);
  }

  // -- connecting and reconnecting --------------------------------------------

  async #first(): Promise<void> {
    const timeout = this.#timeout();
    const deadline = Date.now() + timeout;
    for (;;) {
      try {
        const { connection, slot } = await this.#dialOnce();
        this.#adopt(connection, slot, true);
        return;
      } catch (e) {
        const error = asError(e);
        if (refusal(error) || this.#reconnect === null || Date.now() + this.#delay() > deadline) {
          this.#closed = true;
          this.#failure = error;
          throw error;
        }
        this.onError?.(error);
        await sleep(this.#delay());
        this.#attempt++;
      }
    }
  }

  async #dialOnce(): Promise<{ connection: Connection; slot: Slot }> {
    if (this.#url === null) {
      throw new MqttError("a client opened over a transport cannot dial again", {
        code: MqttErrorCode.Connection,
      });
    }
    const slot: Slot = { connection: null, early: [] };
    const settings: ConnectionSettings = {
      ...this.#options,
      ...(this.#clientId === undefined ? {} : { clientId: this.#clientId }),
    };
    if (this.#level !== null) {
      // A reconnect: the level is the one already negotiated, and Clean Start 0
      // when a session can have survived — so the broker can resume it.
      settings.protocolVersion = this.#level;
      if (this.#resumable()) settings.cleanStart = false;
    }
    let opened = false;
    const dial: Dialer = async (target) => {
      const transport = await this.#dial(target);
      opened = true;
      return transport;
    };
    try {
      const connection = await openWithDial(
        this.#url,
        settings,
        this.#handlers(slot),
        dial,
        this.#inbound,
      );
      return { connection, slot };
    } catch (e) {
      if (opened) this.#ghosts++;
      throw e;
    }
  }

  /** Whether the broker can have kept a session across a disconnect. */
  #resumable(): boolean {
    if (this.#level === 5) return (this.#options.sessionExpiryInterval ?? 0) > 0;
    return this.#options.cleanStart === false;
  }

  #adopt(connection: Connection, slot: Slot, first: boolean): void {
    if (this.#closed) {
      void connection.close().catch(() => {});
      return;
    }
    slot.connection = connection;
    this.#connection = connection;
    this.#level = connection.protocolLevel;
    this.#attempt = 0;
    // A broker-assigned identifier is the session's name from now on.
    const assigned = connection.connack.properties?.assignedClientIdentifier;
    if (assigned !== undefined) this.#clientId = assigned;

    if (!first) {
      if (connection.sessionPresent) this.#resume(connection);
      else this.#rebuild(connection);
    }
    for (const [packet, accept] of slot.early.splice(0)) this.#onMessage(packet, accept);
    this.#ready.resolve(connection);
    if (!first) this.onReconnect?.();
  }

  /**
   * The broker kept the session. 4.4: resend every unacknowledged PUBLISH
   * (DUP set, original identifier) and PUBREL. The subscriptions are still
   * there; unread QoS 1/2 copies are dropped because the broker resends them.
   */
  #resume(connection: Connection): void {
    for (const subscription of this.#subscriptions) subscription.discardReliable();
    for (const [packetId, outbound] of this.#outbound) {
      const packet: Packet =
        outbound.stage === "publish"
          ? { ...outbound.packet, dup: true }
          : { type: PacketType.Pubrel, packetId, reasonCode: 0 };
      void connection.send(packet).catch(() => {});
    }
  }

  /**
   * The broker has no session for us. A publish in flight has an unknown fate,
   * and 4.4 forbids resending it outside a resumed session, so it rejects. The
   * QoS 2 messages we were waiting to see released belong to the old session.
   * Every granted subscription is sent again; a pending one resends itself.
   */
  #rebuild(connection: Connection): void {
    const lost = new MqttError(
      "the broker did not resume the session, so the message's fate is unknown",
      { code: MqttErrorCode.Closed },
    );
    for (const [packetId, waiter] of [...this.#ackWaiters]) {
      if (!this.#outbound.has(packetId)) continue;
      this.#ackWaiters.delete(packetId);
      clearTimeout(waiter.timer);
      waiter.reject(lost);
    }
    this.#inbound.awaitingRelease.clear();
    const seen = new Set<string>();
    for (const subscription of this.#subscriptions) {
      if (!subscription.granted || seen.has(subscription.filter)) continue;
      seen.add(subscription.filter);
      void this.#subscribeOn(
        connection,
        subscription.filter,
        subscription.options,
        Date.now() + this.#timeout(),
      ).catch((e) => this.onError?.(asError(e)));
    }
  }

  #lost(connection: Connection, error: MqttError): void {
    if (this.#connection !== connection) return;
    this.#connection = null;
    this.#ready.reset();
    if (this.#closed) return;
    let final =
      error.code === MqttErrorCode.Server &&
      error.reasonCode !== null &&
      FINAL_DISCONNECTS.has(error.reasonCode);
    // Taken over by one of our own abandoned attempts, not by another client:
    // reconnect, once per such attempt.
    if (final && error.reasonCode === 0x8e && this.#ghosts > 0) {
      this.#ghosts--;
      final = false;
    }
    if (this.#reconnect === null || final) {
      this.#end(error, true);
      return;
    }
    // A SUBSCRIBE or UNSUBSCRIBE in flight will not be answered on a new
    // connection; its own loop sends it again. Publishes wait: whether they are
    // resent or rejected depends on whether the session survives.
    for (const map of [this.#subackWaiters, this.#unsubackWaiters] as Map<
      number,
      Waiter<unknown>
    >[]) {
      for (const waiter of map.values()) {
        clearTimeout(waiter.timer);
        waiter.reject(new Interrupted());
      }
      map.clear();
    }
    this.onDisconnect?.(error);
    this.#schedule();
  }

  #schedule(): void {
    if (this.#closed || this.#timer !== null) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#dialOnce().then(
        ({ connection, slot }) => this.#adopt(connection, slot, false),
        (e: unknown) => {
          this.onError?.(asError(e));
          this.#attempt++;
          this.#schedule();
        },
      );
    }, this.#delay());
  }

  // Exponential backoff with "equal jitter": half the delay fixed, half random.
  #delay(): number {
    const initial = this.#reconnect?.initialDelay ?? DEFAULT_INITIAL_DELAY;
    const max = this.#reconnect?.maxDelay ?? DEFAULT_MAX_DELAY;
    const ceiling = Math.min(max, initial * 2 ** Math.min(this.#attempt, 30));
    return ceiling / 2 + Math.random() * (ceiling / 2);
  }

  /** The live connection, waiting out a reconnect until `deadline`. */
  async #live(deadline: number): Promise<Connection> {
    for (;;) {
      if (this.#closed) throw this.#failure ?? this.#closedError();
      if (this.#connection !== null) return this.#connection;
      const left = deadline - Date.now();
      if (left <= 0) {
        throw new MqttError("not connected within the timeout", { code: MqttErrorCode.Timeout });
      }
      await within(this.#ready.promise, left).catch(() => {});
    }
  }

  #handlers(slot: Slot): ConnectionHandlers {
    const current = () => slot.connection !== null && slot.connection === this.#connection;
    return {
      // The read loop starts with the CONNACK, before this connection is the
      // client's, and a resumed session's backlog arrives right then — already
      // owed an acknowledgement. Held, and handed over on adoption.
      onMessage: (packet, accept) => {
        if (slot.connection === null) slot.early.push([packet, accept]);
        else if (current()) this.#onMessage(packet, accept);
      },
      onAck: (packet) => {
        if (current()) this.#onAck(packet);
      },
      onSuback: (packet) => {
        if (current()) this.#settle(this.#subackWaiters, packet.packetId, packet);
      },
      onUnsuback: (packet) => {
        if (current()) this.#settle(this.#unsubackWaiters, packet.packetId, packet);
      },
      onPingresp: () => {},
      onDisconnect: () => {},
      onClosed: (error) => {
        if (slot.connection !== null && error !== null) this.#lost(slot.connection, error);
      },
    };
  }

  // -- handlers ---------------------------------------------------------------

  #onMessage(packet: PublishPacket, accept: (() => void) | null): void {
    const message = new Message(packet);
    const matching = this.#subscriptions.filter((one) => one.matches(message.topic));
    const ticket = new Ticket(matching.length, accept);
    for (const subscription of matching) subscription.offer(message, ticket);
    if (matching.length === 0) this.onUnmatchedMessage?.(message);
  }

  #onAck(packet: AckPacket): void {
    // PUBACK ends a QoS 1 publish and PUBCOMP ends a successful QoS 2 one. A
    // successful PUBREC moves a QoS 2 exchange to its PUBREL stage — which a
    // resumed session resends instead of the PUBLISH. A negative PUBREC ends it.
    if (packet.type === PacketType.Pubrec) {
      if (!isErrorReason(packet.reasonCode)) {
        const outbound = this.#outbound.get(packet.packetId);
        if (outbound !== undefined) outbound.stage = "pubrel";
        return;
      }
    } else if (packet.type !== PacketType.Puback && packet.type !== PacketType.Pubcomp) {
      return;
    }
    this.#settle(this.#ackWaiters, packet.packetId, packet);
  }

  // -- helpers ----------------------------------------------------------------

  #timeout(): number {
    return this.#options.timeout ?? DEFAULT_TIMEOUT;
  }

  #assertOpen(): void {
    if (this.#closed) throw this.#failure ?? this.#closedError();
  }

  #closedError(): MqttError {
    return new MqttError("the client is closed", { code: MqttErrorCode.Closed });
  }

  async #subscribeOn(
    connection: Connection,
    filter: string,
    options: SubscribeOptions,
    deadline: number,
  ): Promise<SubackPacket> {
    const qos = options.qos ?? 0;
    const packetId = this.#allocatePacketId();
    const entry: SubscribeEntry = { filter, qos };
    if (options.noLocal !== undefined) entry.noLocal = options.noLocal;
    if (options.retainAsPublished !== undefined)
      entry.retainAsPublished = options.retainAsPublished;
    if (options.retainHandling !== undefined) entry.retainHandling = options.retainHandling;
    const properties = this.#subscribeProperties(options);
    const waiter = this.#waitFor(this.#subackWaiters, packetId, Math.max(1, deadline - Date.now()));
    return this.#awaitAck(
      connection,
      connection.send({
        type: PacketType.Subscribe,
        packetId,
        subscriptions: [entry],
        ...(properties === undefined ? {} : { properties }),
      }),
      waiter,
      () => this.#forget(this.#subackWaiters, packetId),
      false,
    );
  }

  #publishProperties(options: PublishOptions): Properties | undefined {
    const merged: Properties = { ...(options.properties ?? {}) };
    if (options.messageExpiryInterval !== undefined)
      merged.messageExpiryInterval = options.messageExpiryInterval;
    if (options.contentType !== undefined) merged.contentType = options.contentType;
    if (options.responseTopic !== undefined) merged.responseTopic = options.responseTopic;
    if (options.correlationData !== undefined) merged.correlationData = options.correlationData;
    if (options.userProperties !== undefined) merged.userProperties = options.userProperties;
    if (Object.keys(merged).length === 0) return undefined;
    this.#assertModern(merged);
    return merged;
  }

  #subscribeProperties(options: SubscribeOptions): Properties | undefined {
    const merged: Properties = { ...(options.properties ?? {}) };
    if (options.subscriptionIdentifier !== undefined) {
      merged.subscriptionIdentifiers = [options.subscriptionIdentifier];
    }
    if (Object.keys(merged).length === 0) return undefined;
    this.#assertModern(merged);
    return merged;
  }

  #assertModern(properties: Properties): void {
    if (this.protocolLevel !== 5 && Object.keys(properties).length > 0) {
      throw new MqttError("5.0 properties need an MQTT 5.0 connection", {
        code: MqttErrorCode.Unsupported,
      });
    }
  }

  /**
   * What the broker's CONNACK forbids a PUBLISH (5.0): a QoS above its Maximum
   * QoS (AWS IoT Core allows 1) and a retained message when Retain Available is
   * 0. The broker would disconnect the client for either; refusing here fails
   * only the one publish.
   */
  #checkPublishAllowed(qos: number, retain: boolean): void {
    const limits = this.#limits();
    if (limits === undefined) return;
    const maxQos = limits.maximumQoS ?? 2;
    if (qos > maxQos) {
      throw new MqttError(
        `the broker accepts QoS ${maxQos} at most; this publish asked for ${qos}`,
        { code: MqttErrorCode.Unsupported },
      );
    }
    if (retain && limits.retainAvailable === 0) {
      throw new MqttError("the broker does not support retained messages", {
        code: MqttErrorCode.Unsupported,
      });
    }
  }

  /** The SUBSCRIBE features a 5.0 broker said it lacks. */
  #checkSubscribeAllowed(filter: string, options: SubscribeOptions): void {
    const limits = this.#limits();
    if (limits === undefined) return;
    const shared = filter.startsWith("$share/");
    if (shared && limits.sharedSubscriptionAvailable === 0) {
      throw new MqttError("the broker does not support shared subscriptions", {
        code: MqttErrorCode.Unsupported,
      });
    }
    const target = shared ? filter.slice(filter.indexOf("/", 7) + 1) : filter;
    if (limits.wildcardSubscriptionAvailable === 0 && /[+#]/.test(target)) {
      throw new MqttError("the broker does not support wildcard subscriptions", {
        code: MqttErrorCode.Unsupported,
      });
    }
    if (
      limits.subscriptionIdentifierAvailable === 0 &&
      (options.subscriptionIdentifier !== undefined ||
        options.properties?.subscriptionIdentifiers !== undefined)
    ) {
      throw new MqttError("the broker does not support subscription identifiers", {
        code: MqttErrorCode.Unsupported,
      });
    }
  }

  #limits(): Properties | undefined {
    const connection = this.#connection;
    return connection !== null && connection.protocolLevel === 5
      ? connection.connack.properties
      : undefined;
  }

  /**
   * Waits for room under the broker's Receive Maximum (5.0): the most QoS 1
   * and 2 publishes it accepts unacknowledged at once (default 65 535). Sending
   * more is a protocol violation it disconnects for (0x93), so a publish past
   * the window waits for one to finish — within its own timeout.
   */
  async #acquireSlot(timeout: number): Promise<void> {
    const maximum = this.#limits()?.receiveMaximum ?? 65535;
    if (this.#inFlight < maximum) {
      this.#inFlight++;
      return;
    }
    await new Promise<void>((resolve, reject) => {
      const grant = () => {
        clearTimeout(timer);
        this.#inFlight++;
        resolve();
      };
      const timer = setTimeout(() => {
        const at = this.#slotWaiters.indexOf(grant);
        if (at >= 0) this.#slotWaiters.splice(at, 1);
        reject(
          new MqttError(
            `no room under the broker's Receive Maximum of ${maximum} within ${timeout}ms`,
            { code: MqttErrorCode.Timeout },
          ),
        );
      }, timeout);
      unrefTimer(timer as unknown as number);
      this.#slotWaiters.push(grant);
    });
  }

  #releaseSlot(): void {
    this.#inFlight--;
    const next = this.#slotWaiters.shift();
    next?.();
  }

  /**
   * The next packet identifier not already waiting on an acknowledgement. The
   * counter wraps at 65 535, and an identifier reused while still in flight
   * would let one acknowledgement settle the wrong request — across a
   * reconnect too, since a resumed session keeps its identifiers.
   */
  #nextPacketId = 1;
  #allocatePacketId(): number {
    for (let tries = 0; tries < 65535; tries++) {
      const id = this.#nextPacketId;
      this.#nextPacketId = id >= 65535 ? 1 : id + 1;
      const busy =
        this.#ackWaiters.has(id) ||
        this.#subackWaiters.has(id) ||
        this.#unsubackWaiters.has(id) ||
        this.#outbound.has(id);
      if (!busy) return id;
    }
    throw new MqttError("every packet identifier is in use", { code: MqttErrorCode.Unsupported });
  }

  /**
   * Awaits a packet's write and its acknowledgement together, settling on
   * whichever fails first.
   *
   * Awaiting the write first would let a write that stalls past the timeout keep
   * the call waiting on a waiter that has already given up. And a timeout that
   * wins while the write is still in flight fails the connection: the packet may
   * yet reach the broker, and the stuck write — held in the connection's
   * serialised chain — would block every later packet, the final DISCONNECT
   * among them. Failing closes the socket and releases the chain.
   *
   * A publish whose write failed because the connection was lost waits on: a
   * resumed session resends it, a fresh one rejects it (see `#rebuild`).
   */
  async #awaitAck<T>(
    connection: Connection,
    write: Promise<void>,
    waiter: Promise<T>,
    onFailure: () => void,
    survivesLoss: boolean,
  ): Promise<T> {
    let written = false;
    const settled = () => {
      written = true;
    };
    void write.then(settled, settled);
    const guarded = write.catch((e: unknown) => {
      if (survivesLoss && this.#reconnect !== null && !this.#closed && connection.closed) return;
      throw e;
    });
    try {
      const [value] = await Promise.all([waiter, guarded]);
      return value;
    } catch (e) {
      onFailure();
      if (!written && !this.#closed && !connection.closed) {
        connection.abort(
          new MqttError("the acknowledgement timed out while the packet was still being written", {
            code: MqttErrorCode.Connection,
          }),
        );
      }
      throw e;
    }
  }

  #waitFor<T>(map: Map<number, Waiter<T>>, packetId: number, timeout: number): Promise<T> {
    const promise = new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        map.delete(packetId);
        reject(new MqttError("the broker did not answer in time", { code: MqttErrorCode.Timeout }));
      }, timeout);
      unrefTimer(timer as unknown as number);
      map.set(packetId, { resolve, reject, timer });
    });
    // Kept handled, so a rejection waits for the caller's `await` instead of
    // tripping the runtime's unhandled-rejection guard.
    void promise.catch(() => {});
    return promise;
  }

  #settle<T>(map: Map<number, Waiter<T>>, packetId: number, value: T): void {
    const waiter = map.get(packetId);
    if (waiter === undefined) return;
    map.delete(packetId);
    clearTimeout(waiter.timer);
    waiter.resolve(value);
  }

  /** Drops a pending waiter without settling it, so its timer cannot reject alone. */
  #forget<T>(map: Map<number, Waiter<T>>, packetId: number): void {
    const waiter = map.get(packetId);
    if (waiter === undefined) return;
    map.delete(packetId);
    clearTimeout(waiter.timer);
  }

  #dropSubscription(subscription: Subscription): void {
    subscription.finish();
    const at = this.#subscriptions.indexOf(subscription);
    if (at >= 0) this.#subscriptions.splice(at, 1);
  }

  #removeSubscriptions(filter: string): void {
    this.#subscriptions = this.#subscriptions.filter((subscription) => {
      if (subscription.filter === filter) {
        subscription.finish();
        return false;
      }
      return true;
    });
  }

  /** Ends the client for good: rejects everything waiting, ends every subscription. */
  #end(error: MqttError, failed: boolean): void {
    if (this.#closed) return;
    this.#closed = true;
    if (failed) this.#failure = error;
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.#ready.reject(error);
    for (const map of [this.#ackWaiters, this.#subackWaiters, this.#unsubackWaiters] as Map<
      number,
      Waiter<unknown>
    >[]) {
      for (const waiter of map.values()) {
        clearTimeout(waiter.timer);
        waiter.reject(error);
      }
      map.clear();
    }
    for (const subscription of this.#subscriptions) subscription.finish();
    this.#subscriptions = [];
  }
}

/** A promise with its settlers, re-armed when the thing it waits for goes away. */
class Signal<T> {
  promise!: Promise<T>;
  #resolve!: (value: T) => void;
  #reject!: (error: Error) => void;
  #settled = true;

  constructor() {
    this.reset();
  }

  reset(): void {
    if (!this.#settled) return;
    this.#settled = false;
    this.promise = new Promise<T>((resolve, reject) => {
      this.#resolve = resolve;
      this.#reject = reject;
    });
    this.promise.catch(() => {});
  }

  resolve(value: T): void {
    this.#settled = true;
    this.#resolve(value);
  }

  reject(error: Error): void {
    this.#settled = true;
    this.#reject(error);
  }
}

/** Settles when `promise` does or `ms` passes, whichever is first. */
function within<T>(promise: Promise<T>, ms: number): Promise<T | undefined> {
  return new Promise<T | undefined>((resolve, reject) => {
    const timer = setTimeout(() => resolve(undefined), ms);
    unrefTimer(timer as unknown as number);
    promise.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      (error) => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

function asError(e: unknown): MqttError {
  return e instanceof MqttError
    ? e
    : new MqttError(e instanceof Error ? e.message : String(e), {
        code: MqttErrorCode.Connection,
        cause: e,
      });
}

function matches(a: Uint8Array | null, b: Uint8Array): boolean {
  if (a === null || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function randomBytes(length: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(length));
}

function randomToken(): string {
  return Array.from(randomBytes(8), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
