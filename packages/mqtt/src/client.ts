/**
 * The client: publish, subscribe, and request/reply over one connection.
 *
 * Everything the connection answered on the protocol's behalf — PUBACK for an
 * inbound QoS 1 message, the PUBREC/PUBREL/PUBCOMP exchange in either direction,
 * PINGREQ — has already happened by the time a {@link Message} arrives here. What
 * is left is what an application asked for:
 *
 * - **Publish** at QoS 0, 1 or 2, where the QoS 1 and 2 calls resolve only when
 *   the broker has acknowledged (or refuse, with the broker's reason code, when
 *   it would not take the message).
 * - **Subscribe**, which registers a topic filter and hands back an async
 *   iterable of {@link Message}; the connection's messages are routed to it by
 *   the filter, and the two wildcards are matched here, not guessed at.
 * - **Request/reply**, which is the 5.0 convention and nothing more: publish
 *   with a response topic and correlation data, and read the first reply that
 *   carries the same correlation data back.
 *
 * QoS 1 and 2 delivery to the broker is at-least-once and exactly-once as far as
 * the protocol is concerned; the acknowledgement is what this layer waits on, and
 * a publish that never draws one fails with a timeout rather than hanging.
 * Redelivery of a QoS 1 or 2 message after a reconnect is the application's
 * decision — this client does not redial on its own — and `dup` is set on a
 * message the broker says is a retransmission.
 */

import { unrefTimer } from "runtime:process";
import { Connection, type ConnectionHandlers, type ConnectionSettings } from "./connection.js";
import { MqttError, MqttErrorCode } from "./errors.js";
import {
  type AckPacket,
  PacketType,
  type PublishPacket,
  type SubackPacket,
  type SubscribeEntry,
  type UnsubackPacket,
} from "./protocol/packets.js";
import type { Properties } from "./protocol/properties.js";
import { isErrorReason, reasonName } from "./protocol/reason.js";
import { topicMatches, validateTopicName } from "./protocol/topic.js";
import type { Transport } from "./transport.js";

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
  /** How long to wait for the acknowledgement, in ms. Defaults 30 000. */
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
  /** How long to wait for the SUBACK, in ms. Defaults 30 000. */
  timeout?: number;
}

/** Any extra connection settings, plus a default operation timeout. */
export interface MqttClientOptions extends ConnectionSettings {
  /** The default timeout for a publish/subscribe, in ms. Defaults 30 000. */
  timeout?: number;
}

/** A live subscription: an async iterable of messages, until unsubscribed. */
export class Subscription implements AsyncIterable<Message> {
  readonly filter: string;
  #client: MqttClient;
  #queue: Message[] = [];
  #waiters: Array<(result: IteratorResult<Message>) => void> = [];
  #closed = false;
  #grantedQos: 0 | 1 | 2 = 0;

  /** @internal */
  constructor(client: MqttClient, filter: string) {
    this.#client = client;
    this.filter = filter;
  }

  /** The QoS the broker granted for this filter. */
  get grantedQos(): 0 | 1 | 2 {
    return this.#grantedQos;
  }

  /** Whether the subscription is over. */
  get closed(): boolean {
    return this.#closed;
  }

  /** @internal */
  setGrantedQos(qos: 0 | 1 | 2): void {
    this.#grantedQos = qos;
  }

  /** @internal */
  offer(message: Message): void {
    if (this.#closed) return;
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) waiter({ value: message, done: false });
    else this.#queue.push(message);
  }

  /** @internal */
  finish(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const waiter of this.#waiters.splice(0)) waiter({ value: undefined, done: true });
  }

  /** Whether `topic` matches this subscription's filter. */
  matches(topic: string): boolean {
    return topicMatches(this.filter, topic);
  }

  /** The next message, or `done` once the subscription ends. */
  next(): Promise<IteratorResult<Message>> {
    const message = this.#queue.shift();
    if (message !== undefined) return Promise.resolve({ value: message, done: false });
    if (this.#closed) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve) => this.#waiters.push(resolve));
  }

  /** The next message, rejecting if none arrives within `timeout` ms. */
  nextMessage(timeout: number = DEFAULT_TIMEOUT): Promise<Message> {
    const message = this.#queue.shift();
    if (message !== undefined) return Promise.resolve(message);
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

  /** @internal */
  #removeWaiter(waiter: (result: IteratorResult<Message>) => void): boolean {
    const at = this.#waiters.indexOf(waiter);
    if (at < 0) return false;
    this.#waiters.splice(at, 1);
    return true;
  }

  /** Unsubscribes from the broker and ends the iterable. */
  async unsubscribe(): Promise<void> {
    if (this.#closed) return;
    await this.#client.unsubscribe(this.filter);
    this.finish();
  }

  [Symbol.asyncIterator](): AsyncIterator<Message> {
    return this;
  }
}

type Waiter<T> = {
  resolve: (value: T) => void;
  reject: (error: MqttError) => void;
  timer: ReturnType<typeof setTimeout>;
};

export class MqttClient {
  #connection: Connection;
  #options: MqttClientOptions;
  #subscriptions: Subscription[] = [];
  #ackWaiters = new Map<number, Waiter<AckPacket>>();
  #subackWaiters = new Map<number, Waiter<SubackPacket>>();
  #unsubackWaiters = new Map<number, Waiter<UnsubackPacket>>();
  #closed = false;

  private constructor(connection: Connection, options: MqttClientOptions) {
    this.#connection = connection;
    this.#options = options;
  }

  /** Connects to `url` and completes the handshake. */
  static async connect(url: string, options: MqttClientOptions = {}): Promise<MqttClient> {
    const holder: { client: MqttClient | null } = { client: null };
    const connection = await Connection.open(url, options, MqttClient.#handlers(holder));
    const client = new MqttClient(connection, options);
    holder.client = client;
    return client;
  }

  /**
   * Connects over an already-established transport.
   *
   * The seam a test stands on: the client's routing, acknowledgement handling
   * and request/reply run the same whether the bytes come from a socket or a
   * scripted broker in memory.
   */
  static async over(
    transport: Transport,
    options: MqttClientOptions = {},
    level: 4 | 5 = 5,
  ): Promise<MqttClient> {
    const holder: { client: MqttClient | null } = { client: null };
    const connection = await Connection.over(
      transport,
      options,
      MqttClient.#handlers(holder),
      level,
    );
    const client = new MqttClient(connection, options);
    holder.client = client;
    return client;
  }

  static #handlers(holder: { client: MqttClient | null }): ConnectionHandlers {
    return {
      onMessage: (packet) => {
        if (holder.client !== null) holder.client.#onMessage(packet);
      },
      onAck: (packet) => {
        if (holder.client !== null) holder.client.#onAck(packet);
      },
      onSuback: (packet) => {
        if (holder.client !== null) holder.client.#onSuback(packet);
      },
      onUnsuback: (packet) => {
        if (holder.client !== null) holder.client.#onUnsuback(packet);
      },
      onPingresp: () => {},
      onDisconnect: () => {},
      onClosed: (error) => {
        if (holder.client !== null) holder.client.#onClosed(error);
      },
    };
  }

  /** The negotiated protocol level: 5 for MQTT 5.0, 4 for 3.1.1. */
  get protocolLevel(): 4 | 5 {
    return this.#connection.protocolLevel;
  }

  /** Whether the broker kept a session for this client. */
  get sessionPresent(): boolean {
    return this.#connection.sessionPresent;
  }

  /** Whether the client is closed. */
  get closed(): boolean {
    return this.#closed;
  }

  /** The failure that ended the connection, or `null` while it stands. */
  get failure(): MqttError | null {
    return this.#connection.failure;
  }

  /** Sends a PINGREQ and resolves when the PINGRESP arrives. */
  ping(timeout?: number): Promise<void> {
    return this.#connection.ping(timeout);
  }

  /** Publishes a message, resolving once the broker has acknowledged it. */
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

    if (qos === 0) {
      await this.#connection.send({
        type: PacketType.Publish,
        topic,
        payload: bytes,
        qos: 0,
        retain,
        dup: false,
        ...(properties === undefined ? {} : { properties }),
      });
      return;
    }

    const packetId = this.#connection.allocatePacketId();
    // QoS 1 waits for PUBACK; QoS 2 waits for PUBCOMP, the connection having
    // answered the PUBREC in between.
    const waiter = this.#waitFor(this.#ackWaiters, packetId, options.timeout ?? this.#timeout());
    const ack = await this.#awaitAck(
      this.#connection.send({
        type: PacketType.Publish,
        topic,
        payload: bytes,
        qos,
        retain,
        dup: false,
        packetId,
        ...(properties === undefined ? {} : { properties }),
      }),
      waiter,
      // A write that never left — an unencodable packet, or one the connection
      // refused — cannot leave its timer to reject alone.
      () => this.#forget(this.#ackWaiters, packetId),
    );
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

  /** Subscribes to a topic filter and returns the messages it matches. */
  async subscribe(filter: string, options: SubscribeOptions = {}): Promise<Subscription> {
    this.#assertOpen();
    const qos = options.qos ?? 0;
    const packetId = this.#connection.allocatePacketId();
    const subscription = new Subscription(this, filter);
    // Register before the SUBACK, so a message that arrives with the grant is
    // routed rather than dropped.
    this.#subscriptions.push(subscription);

    const entry: SubscribeEntry = { filter, qos };
    if (options.noLocal !== undefined) entry.noLocal = options.noLocal;
    if (options.retainAsPublished !== undefined)
      entry.retainAsPublished = options.retainAsPublished;
    if (options.retainHandling !== undefined) entry.retainHandling = options.retainHandling;
    const properties = this.#subscribeProperties(options);

    const waiter = this.#waitFor(this.#subackWaiters, packetId, options.timeout ?? this.#timeout());
    const ack = await this.#awaitAck(
      this.#connection.send({
        type: PacketType.Subscribe,
        packetId,
        subscriptions: [entry],
        ...(properties === undefined ? {} : { properties }),
      }),
      waiter,
      // A send failure or a SUBACK that never came: drop the waiter and the
      // subscription together, or the abandoned subscription keeps buffering
      // every matching message with no one left to read them.
      () => {
        this.#forget(this.#subackWaiters, packetId);
        this.#dropSubscription(subscription);
      },
    );
    const reason = ack.reasons[0];
    if (reason === undefined || isErrorReason(reason)) {
      this.#dropSubscription(subscription);
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
  }

  /** Unsubscribes from one or more topic filters. */
  async unsubscribe(filters: string | string[], timeout?: number): Promise<void> {
    this.#assertOpen();
    const list = Array.isArray(filters) ? filters : [filters];
    const packetId = this.#connection.allocatePacketId();
    const waiter = this.#waitFor(this.#unsubackWaiters, packetId, timeout ?? this.#timeout());
    await this.#awaitAck(
      this.#connection.send({ type: PacketType.Unsubscribe, packetId, filters: list }),
      waiter,
      () => this.#forget(this.#unsubackWaiters, packetId),
    );
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
    if (this.#connection.protocolLevel !== 5) {
      throw new MqttError("request/reply needs MQTT 5.0", { code: MqttErrorCode.Unsupported });
    }
    const responseTopic = options.responseTopic ?? `$esrun/reply/${randomToken()}`;
    const correlationData = options.correlationData ?? randomBytes(8);
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

  /** Closes the connection, ending every subscription. */
  async close(options?: { reasonCode?: number; properties?: Properties }): Promise<void> {
    if (this.#closed) return;
    this.#finish(new MqttError("the client was closed", { code: MqttErrorCode.Closed }));
    await this.#connection.close(options ?? {});
  }

  // -- handlers -------------------------------------------------------------

  #onMessage(packet: PublishPacket): void {
    const message = new Message(packet);
    for (const subscription of this.#subscriptions) {
      if (subscription.matches(message.topic)) subscription.offer(message);
    }
  }

  #onAck(packet: AckPacket): void {
    // PUBACK ends a QoS 1 publish and PUBCOMP ends a successful QoS 2 one. A
    // PUBREC is only news when it is negative: the connection sends no PUBREL
    // then, so the publish fails here rather than waiting out a PUBCOMP that
    // will never come.
    if (packet.type === PacketType.Pubrec) {
      if (!isErrorReason(packet.reasonCode)) return;
    } else if (packet.type !== PacketType.Puback && packet.type !== PacketType.Pubcomp) {
      return;
    }
    const waiter = this.#ackWaiters.get(packet.packetId);
    if (waiter === undefined) return;
    this.#settle(this.#ackWaiters, packet.packetId, packet);
  }

  #onSuback(packet: SubackPacket): void {
    this.#settle(this.#subackWaiters, packet.packetId, packet);
  }

  #onUnsuback(packet: UnsubackPacket): void {
    this.#settle(this.#unsubackWaiters, packet.packetId, packet);
  }

  #onClosed(error: MqttError | null): void {
    this.#finish(
      error ?? new MqttError("the connection was closed", { code: MqttErrorCode.Closed }),
    );
  }

  // -- helpers --------------------------------------------------------------

  #timeout(): number {
    return this.#options.timeout ?? DEFAULT_TIMEOUT;
  }

  #assertOpen(): void {
    if (this.#closed) throw new MqttError("the client is closed", { code: MqttErrorCode.Closed });
    if (this.#connection.failure !== null) throw this.#connection.failure;
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
    if (this.#connection.protocolLevel !== 5 && Object.keys(properties).length > 0) {
      throw new MqttError("5.0 properties need an MQTT 5.0 connection", {
        code: MqttErrorCode.Unsupported,
      });
    }
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
   * A deliberate close is not that case. It rejects the waiter itself, before the
   * write stops being written, and it tears the connection down on its own — by
   * sending the DISCONNECT and closing the socket, not by failing. Aborting here
   * would race that path, turn the close into a connection failure and skip the
   * DISCONNECT, so it is left to the close.
   */
  async #awaitAck<T>(write: Promise<void>, waiter: Promise<T>, onFailure: () => void): Promise<T> {
    let written = false;
    const settled = () => {
      written = true;
    };
    void write.then(settled, settled);
    try {
      const [value] = await Promise.all([waiter, write]);
      return value;
    } catch (e) {
      onFailure();
      if (!written && !this.#closed) {
        this.#connection.abort(
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
    // The timer starts here, but every caller awaits this only after its `send`
    // has resolved. A write slower than the timeout therefore rejects the waiter
    // while the caller is still awaiting the socket, with nobody yet awaiting
    // it. Kept handled, so the rejection waits for the caller's `await` instead
    // of tripping the runtime's unhandled-rejection guard.
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

  #finish(error: MqttError): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const waiter of this.#ackWaiters.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    this.#ackWaiters.clear();
    for (const waiter of this.#subackWaiters.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    this.#subackWaiters.clear();
    for (const waiter of this.#unsubackWaiters.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    this.#unsubackWaiters.clear();
    for (const subscription of this.#subscriptions) subscription.finish();
    this.#subscriptions = [];
  }
}

function matches(a: Uint8Array | null, b: Uint8Array): boolean {
  if (a === null || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

function randomBytes(length: number): Uint8Array {
  const out = new Uint8Array(length);
  for (let i = 0; i < length; i++) out[i] = Math.floor(Math.random() * 256);
  return out;
}

function randomToken(): string {
  return Array.from(randomBytes(8), (byte) => byte.toString(16).padStart(2, "0")).join("");
}
