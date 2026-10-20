/**
 * A channel: the AMQP session a program actually works in.
 *
 * Everything an application does to a broker — declaring a queue, publishing a
 * message, acknowledging one, asking for confirms — happens on a channel, and a
 * connection carries many. This is where a method and its answer are paired up,
 * where a delivery's method, header and body frames become one message, and
 * where a broker's refusal becomes a thrown error.
 *
 * The shape of it follows from two facts about AMQP:
 *
 * - **A request and its answer are separate frames.** `queue.declare` is
 *   answered by `queue.declare-ok` some time later, on the same channel, with
 *   other traffic possibly in between. So every call that has an answer
 *   registers a waiter for that answer *before* it sends, and this channel —
 *   driven by {@link ChannelHandler} callbacks from the connection — is what
 *   hands the answer back.
 * - **Content is three frames, not one.** A publish or a delivery is a method,
 *   then a header saying how many body octets follow, then the body. This
 *   channel reassembles them into a {@link Message}, because a layer above
 *   should never have to think about frames.
 *
 * What is deliberately *not* here: subscriptions and their flow control, the
 * request/reply convenience over `direct reply-to`, and pooling. Those are
 * policy — how many messages to hold, whether to re-consume after a reconnect —
 * and they sit above a channel rather than inside it. This layer is the
 * vocabulary; the layers above decide when to speak.
 */

import type { ChannelHandler, Connection } from "./connection.js";
import { ProtocolError, RabbitError, RabbitErrorCode } from "./errors.js";
import type { FieldTable } from "./protocol/fields.js";
import type { DecodedMethod, MethodArg } from "./protocol/methods.js";
import type { BasicProperties, ContentHeader } from "./protocol/properties.js";

/** How long a method waits for its answer, in ms, unless told otherwise. */
const DEFAULT_TIMEOUT = 30_000;

/**
 * A message, as the broker or a peer described it: the delivery's envelope, the
 * properties the sender set, and the body.
 */
export interface Message {
  /** The consumer that received it; set on a delivery, unset on a `get` or a return. */
  consumerTag?: string;
  /** The broker's handle for this delivery, for `ack`/`nack`/`reject`. */
  deliveryTag: bigint;
  /** Whether the broker has delivered this message before. */
  redelivered: boolean;
  exchange: string;
  routingKey: string;
  properties: BasicProperties;
  /** The body octets, exactly as sent. */
  body: Uint8Array;
  /** Set when the broker returned the message because it could not be routed. */
  returned?: { replyCode: number; replyText: string };
  /** How many messages were left in the queue, from a `basic.get-ok`. */
  messageCount?: number;
}

/**
 * A message that arrived from a subscription, with the ways to settle it.
 *
 * It is a {@link Message} — the envelope, properties and body — plus `ack`,
 * `nack` and `reject`, which name this delivery's tag so the caller never has
 * to carry it around.
 */
export interface Delivery extends Message {
  ack(options?: AckOptions): Promise<void>;
  nack(options?: NackOptions): Promise<void>;
  reject(options?: RejectOptions): Promise<void>;
}

export interface ExchangeOptions {
  passive?: boolean;
  durable?: boolean;
  autoDelete?: boolean;
  /** A broker-side exchange that publishes to, but is not fed by, publishers. */
  internal?: boolean;
  arguments?: FieldTable;
}

export interface QueueOptions {
  passive?: boolean;
  durable?: boolean;
  /** Only this connection may use it; it is deleted when the connection ends. */
  exclusive?: boolean;
  autoDelete?: boolean;
  arguments?: FieldTable;
}

export interface BindOptions {
  arguments?: FieldTable;
}

export interface QueueDeleteOptions {
  ifUnused?: boolean;
  ifEmpty?: boolean;
}

export interface ExchangeDeleteOptions {
  ifUnused?: boolean;
}

export interface QosOptions {
  /** In octets; RabbitMQ does not implement this and only accepts 0. */
  prefetchSize?: number;
  /** Messages unacknowledged at once; 0 means no limit. */
  prefetchCount?: number;
  /**
   * As RabbitMQ reinterprets the flag: `false` (the default) limits each new
   * consumer on the channel on its own; `true` limits the channel as a whole,
   * shared by its consumers. (The AMQP specification's per-channel versus
   * per-connection meaning is not what RabbitMQ implements.)
   */
  global?: boolean;
}

/** What a `queue.declare` reports back; the name is the broker's if none was given. */
export interface QueueDeclaration {
  queue: string;
  messageCount: number;
  consumerCount: number;
}

/** A message and how it arrived, before the body is assembled. */
interface Fulfilment {
  method: DecodedMethod;
  message: Message | null;
}

interface Waiter {
  names: readonly string[];
  resolve: (fulfilment: Fulfilment) => void;
  reject: (error: RabbitError) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

interface Confirmation {
  promise: Promise<void>;
  resolve: () => void;
  reject: (error: RabbitError) => void;
}

/**
 * One channel over one connection.
 *
 * It is handed to {@link Connection} as that channel's {@link ChannelHandler},
 * so the connection does the reading and this does the interpreting. The public
 * methods are asynchronous and each answers with what the protocol answers with
 * — a declaration's generated name, a `get`'s message or `null` — rather than a
 * method frame, which is the whole point of the layer.
 */
export class Channel implements ChannelHandler {
  readonly #connection: Connection;
  readonly #timeout: number;
  #number = 0;

  #waiters: Waiter[] = [];

  // The content state machine: `basic.deliver` and its like announce a body,
  // which then arrives as a header frame and one or more body frames. These
  // hold the half-assembled message in between.
  #method: DecodedMethod | null = null;
  #header: ContentHeader | null = null;
  #chunks: Uint8Array[] = [];
  #bodySize = 0;
  #received = 0;

  // Publisher confirms. `#nextSeq` is the delivery tag the broker will use for
  // the next publish; `#outstanding` is what has been sent and not yet answered.
  #confirmMode = false;
  #nextSeq = 1n;
  #outstanding = new Map<bigint, Confirmation>();

  #consumerPrefetch: number | null = null;

  #closed = false;
  #closeError: RabbitError | null = null;
  #closedSignal: Promise<RabbitError | null>;
  #resolveClosed!: (error: RabbitError | null) => void;

  /** A delivery, from a subscription. Set by the consumer layer. */
  onDelivery: ((message: Message) => void) | null = null;
  /** The broker cancelled a consumer (`consumer_cancel_notify`). */
  onCancel: ((consumerTag: string) => void) | null = null;
  /**
   * The broker returned a message it could not route, because it was published
   * with `mandatory`. The message has not been delivered anywhere.
   */
  onReturn: ((message: Message) => void) | null = null;
  /** The channel ended. `null` when it was closed on purpose. */
  onClose: ((error: RabbitError | null) => void) | null = null;

  constructor(connection: Connection, options: { timeout?: number } = {}) {
    this.#connection = connection;
    this.#timeout = options.timeout ?? DEFAULT_TIMEOUT;
    this.#closedSignal = new Promise((resolve) => {
      this.#resolveClosed = resolve;
    });
  }

  /** The channel's number on the connection. Set when it is opened. */
  get number(): number {
    return this.#number;
  }

  /** Whether the channel can still be used. */
  get usable(): boolean {
    return !this.#closed && this.#closeError === null;
  }

  /** What ended the channel, if the broker is the one that ended it. */
  get failure(): RabbitError | null {
    return this.#closeError;
  }

  /** Called by the connection once it has allocated the channel's number. */
  attach(number: number): void {
    this.#number = number;
  }

  // -- declaring topology ---------------------------------------------------

  /** Declares an exchange; declaring one that exists is fine if it matches. */
  async exchangeDeclare(name: string, type: string, options: ExchangeOptions = {}): Promise<void> {
    await this.#request(
      "exchange.declare",
      {
        exchange: name,
        type,
        passive: options.passive ?? false,
        durable: options.durable ?? false,
        autoDelete: options.autoDelete ?? false,
        internal: options.internal ?? false,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["exchange.declare-ok"],
    );
  }

  /** Deletes an exchange. */
  async exchangeDelete(name: string, options: ExchangeDeleteOptions = {}): Promise<void> {
    await this.#request(
      "exchange.delete",
      { exchange: name, ifUnused: options.ifUnused ?? false },
      ["exchange.delete-ok"],
    );
  }

  /** Binds one exchange to another: messages `source` accepts also go to `destination`. */
  async exchangeBind(
    destination: string,
    source: string,
    routingKey = "",
    options: BindOptions = {},
  ): Promise<void> {
    await this.#request(
      "exchange.bind",
      {
        destination,
        source,
        routingKey,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["exchange.bind-ok"],
    );
  }

  /** Undoes an {@link exchangeBind}. */
  async exchangeUnbind(
    destination: string,
    source: string,
    routingKey = "",
    options: BindOptions = {},
  ): Promise<void> {
    await this.#request(
      "exchange.unbind",
      {
        destination,
        source,
        routingKey,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["exchange.unbind-ok"],
    );
  }

  /**
   * Declares a queue and returns what the broker says about it.
   *
   * An empty `name` asks the broker to name it, and the generated name is the
   * one in the answer — which is why this returns a declaration rather than
   * nothing.
   */
  async queueDeclare(name = "", options: QueueOptions = {}): Promise<QueueDeclaration> {
    const { method } = await this.#request(
      "queue.declare",
      {
        queue: name,
        passive: options.passive ?? false,
        durable: options.durable ?? false,
        exclusive: options.exclusive ?? false,
        autoDelete: options.autoDelete ?? false,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["queue.declare-ok"],
    );
    return {
      queue: String(method.args.queue),
      messageCount: asNumber(method.args.messageCount),
      consumerCount: asNumber(method.args.consumerCount),
    };
  }

  /** Binds a queue to an exchange. */
  async queueBind(
    queue: string,
    exchange: string,
    routingKey = "",
    options: BindOptions = {},
  ): Promise<void> {
    await this.#request(
      "queue.bind",
      {
        queue,
        exchange,
        routingKey,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["queue.bind-ok"],
    );
  }

  /** Undoes a {@link queueBind}. */
  async queueUnbind(
    queue: string,
    exchange: string,
    routingKey = "",
    options: BindOptions = {},
  ): Promise<void> {
    await this.#request(
      "queue.unbind",
      {
        queue,
        exchange,
        routingKey,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["queue.unbind-ok"],
    );
  }

  /** Empties a queue, and answers with how many messages it dropped. */
  async queuePurge(queue: string): Promise<number> {
    const { method } = await this.#request("queue.purge", { queue }, ["queue.purge-ok"]);
    return asNumber(method.args.messageCount);
  }

  /** Deletes a queue, and answers with how many messages it held. */
  async queueDelete(queue: string, options: QueueDeleteOptions = {}): Promise<number> {
    const { method } = await this.#request(
      "queue.delete",
      { queue, ifUnused: options.ifUnused ?? false, ifEmpty: options.ifEmpty ?? false },
      ["queue.delete-ok"],
    );
    return asNumber(method.args.messageCount);
  }

  // -- quality of service ---------------------------------------------------

  /** Limits how much the broker will send before an acknowledgement. */
  async qos(options: QosOptions = {}): Promise<void> {
    const prefetchCount = options.prefetchCount ?? 0;
    const global = options.global ?? false;
    await this.#request(
      "basic.qos",
      { prefetchSize: options.prefetchSize ?? 0, prefetchCount, global },
      ["basic.qos-ok"],
    );
    if (!global) this.#consumerPrefetch = prefetchCount;
  }

  /**
   * The per-consumer prefetch in force for consumers started from now on — the
   * last `qos({ global: false })` the broker accepted — or `null` if none was
   * ever set, which RabbitMQ treats as unlimited.
   */
  get consumerPrefetch(): number | null {
    return this.#consumerPrefetch;
  }

  // -- publishing -----------------------------------------------------------

  /**
   * Publishes a message.
   *
   * Without confirms this resolves once the frames have been written, which is
   * all the protocol promises. With {@link confirmSelect} in force it resolves
   * once the broker has acknowledged the message, and rejects if the broker
   * cannot take it — so awaiting the publish is awaiting its fate.
   */
  async publish(
    exchange: string,
    routingKey: string,
    body: string | Uint8Array,
    options: PublishOptions = {},
  ): Promise<void> {
    this.#requireOpen();
    const bytes = typeof body === "string" ? new TextEncoder().encode(body) : body;
    const { mandatory = false, ...properties } = options;
    const sequence = this.#confirmMode ? this.#nextSeq++ : 0n;
    const confirmation = this.#confirmMode ? this.#track(sequence) : null;
    try {
      await this.#connection.sendContent(
        this.#number,
        "basic.publish",
        { exchange, routingKey, mandatory },
        properties,
        bytes,
      );
    } catch (e) {
      // The message never left, so no confirm will ever settle it; drop the
      // promise rather than leave a rejection nobody is holding.
      if (confirmation !== null) {
        this.#outstanding.delete(sequence);
        confirmation.catch(() => {});
      }
      throw e;
    }
    if (confirmation !== null) await confirmation;
  }

  /**
   * Puts the channel in confirm mode: from here the broker acknowledges every
   * publish, and {@link publish} does not resolve until it has.
   */
  async confirmSelect(): Promise<void> {
    // The broker numbers publishes from the `confirm.select` itself, not from
    // its `select-ok`: a publish sent in between is already its number 1. So
    // confirm mode starts when the request is sent, or that publish would go
    // untracked and every later confirm would settle the wrong message.
    // Selecting twice is allowed and does not restart the count.
    const first = !this.#confirmMode;
    this.#confirmMode = true;
    if (first) this.#nextSeq = 1n;
    await this.#request("confirm.select", {}, ["confirm.select-ok"]);
  }

  /** Whether every publish so far has been confirmed; rejects on the first nack. */
  async waitForConfirms(): Promise<void> {
    if (!this.#confirmMode) {
      throw new RabbitError("publisher confirms are not enabled on this channel", {
        code: RabbitErrorCode.Channel,
      });
    }
    while (this.#outstanding.size > 0) {
      await Promise.all([...this.#outstanding.values()].map((one) => one.promise));
    }
  }

  // -- getting and acknowledging --------------------------------------------

  /** Fetches one message, or `null` when the queue is empty. */
  async get(queue: string, options: GetOptions = {}): Promise<Message | null> {
    const { method, message } = await this.#request(
      "basic.get",
      { queue, noAck: options.noAck ?? false },
      ["basic.get-ok", "basic.get-empty"],
    );
    return method.name === "basic.get-empty" ? null : message;
  }

  /** Acknowledges a delivery. */
  async ack(deliveryTag: bigint | number, options: AckOptions = {}): Promise<void> {
    await this.#send("basic.ack", {
      deliveryTag: asTag(deliveryTag),
      multiple: options.multiple ?? false,
    });
  }

  /** Rejects a delivery, with the option of requeuing it. */
  async nack(deliveryTag: bigint | number, options: NackOptions = {}): Promise<void> {
    await this.#send("basic.nack", {
      deliveryTag: asTag(deliveryTag),
      multiple: options.multiple ?? false,
      requeue: options.requeue ?? true,
    });
  }

  /** Rejects one delivery; `requeue` says whether it goes back on the queue. */
  async reject(deliveryTag: bigint | number, options: RejectOptions = {}): Promise<void> {
    await this.#send("basic.reject", {
      deliveryTag: asTag(deliveryTag),
      requeue: options.requeue ?? true,
    });
  }

  /** Asks the broker to redeliver everything unacknowledged on this channel. */
  async recover(requeue = true): Promise<void> {
    await this.#request("basic.recover", { requeue }, ["basic.recover-ok"]);
  }

  // -- consuming (the vocabulary the consumer layer builds on) --------------

  /**
   * Starts a consumer on a queue and answers with its tag.
   *
   * A tag is always sent, chosen here if the caller did not choose one, because
   * a delivery names the consumer it was for: knowing the tag *before* the
   * first delivery can arrive is what lets the consumer layer route one without
   * a race.
   */
  async consume(
    queue: string,
    options: {
      consumerTag?: string;
      noAck?: boolean;
      exclusive?: boolean;
      arguments?: FieldTable;
    } = {},
  ): Promise<string> {
    const consumerTag = options.consumerTag ?? "";
    const { method } = await this.#request(
      "basic.consume",
      {
        queue,
        consumerTag,
        noLocal: false,
        noAck: options.noAck ?? false,
        exclusive: options.exclusive ?? false,
        ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
      },
      ["basic.consume-ok"],
    );
    return String(method.args.consumerTag);
  }

  /** Cancels a consumer and answers once the broker has stopped it. */
  async cancelConsumer(consumerTag: string): Promise<void> {
    await this.#request("basic.cancel", { consumerTag, noWait: false }, ["basic.cancel-ok"]);
  }

  /**
   * Resolves when the channel ends, with the reason, or `null` when it was
   * closed on purpose. It is a promise rather than a hook so that a subscriber
   * can wait for the end without taking `onClose` away from whoever set it.
   */
  whenClosed(): Promise<RabbitError | null> {
    return this.#closedSignal;
  }

  // -- closing --------------------------------------------------------------

  /** Closes the channel, and answers once the broker has agreed. */
  async close(): Promise<void> {
    if (this.#closed) return;
    await this.#connection.closeChannel(this.#number);
  }

  // -- what the connection calls --------------------------------------------

  onMethod(_channel: number, method: DecodedMethod): void {
    // A message's method, header and body frames are contiguous on a channel
    // (AMQP 0-9-1 §4.2.6); any method arriving inside one is the broker's
    // framing and ours disagreeing, which the spec makes 505 UNEXPECTED_FRAME.
    // Throwing here fails the connection, as a framing error must.
    if (this.#method !== null) {
      throw new ProtocolError(
        `${method.name} arrived while ${this.#method.name}'s content was incomplete`,
      );
    }
    if (method.hasContent) {
      this.#method = method;
      this.#header = null;
      this.#chunks = [];
      this.#bodySize = 0;
      this.#received = 0;
      return;
    }
    this.#dispatch({ method, message: null });
  }

  onHeader(_channel: number, header: ContentHeader): void {
    if (this.#method === null) {
      throw new ProtocolError("a content header arrived without a method to announce it");
    }
    if (this.#header !== null) {
      throw new ProtocolError("a second content header arrived for one message");
    }
    this.#header = header;
    this.#bodySize = asNumber(header.bodySize);
    if (this.#bodySize === 0) this.#complete();
  }

  onBody(_channel: number, bytes: Uint8Array): void {
    if (this.#method === null || this.#header === null) {
      throw new ProtocolError("a body frame arrived without a content header");
    }
    if (this.#received + bytes.length > this.#bodySize) {
      throw new ProtocolError("a body frame overruns the content header's body size");
    }
    this.#chunks.push(bytes);
    this.#received += bytes.length;
    if (this.#received >= this.#bodySize) this.#complete();
  }

  onClosed(_channel: number, error: RabbitError | null): void {
    this.#closed = true;
    // A channel this side abandoned (a request that timed out) already holds
    // its reason; the `close-ok` that follows must not replace it with `null`.
    if (this.#closeError === null) this.#closeError = error;
    error = this.#closeError;
    const reason =
      error ?? new RabbitError("the channel was closed", { code: RabbitErrorCode.Closed });
    for (const waiter of this.#waiters.splice(0)) {
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      waiter.reject(reason);
    }
    for (const [, confirmation] of this.#outstanding) confirmation.reject(reason);
    this.#outstanding.clear();
    this.#method = null;
    this.#header = null;
    this.#chunks = [];
    this.#resolveClosed(error);
    this.onClose?.(error);
  }

  // -- the request/answer pairing -------------------------------------------

  async #request(
    name: string,
    args: Record<string, MethodArg>,
    answers: readonly string[],
  ): Promise<Fulfilment> {
    const waiting = this.#expect(answers);
    try {
      await this.#connection.send(this.#number, name, args);
    } catch (e) {
      // The send failed, so nothing will answer: drop the waiter rather than
      // leave a rejection nobody is holding.
      waiting.catch(() => {});
      throw e;
    }
    return waiting;
  }

  #expect(names: readonly string[]): Promise<Fulfilment> {
    this.#requireOpen();
    return new Promise((resolve, reject) => {
      const waiter: Waiter = { names, resolve, reject, timer: null };
      if (this.#timeout > 0) {
        waiter.timer = setTimeout(() => {
          const at = this.#waiters.indexOf(waiter);
          if (at !== -1) this.#waiters.splice(at, 1);
          const error = new RabbitError(
            `no ${names.join(" or ")} from the broker within ${this.#timeout}ms`,
            { code: RabbitErrorCode.Timeout },
          );
          reject(error);
          this.#abandon(error);
        }, this.#timeout);
      }
      this.#waiters.push(waiter);
    });
  }

  /**
   * Gives up on the channel after a request went unanswered. Answers are paired
   * with requests by order, so once one is missing a late answer would settle
   * the *next* request of the same kind — a second `queueDeclare` resolving
   * with the first one's queue. The channel is closed instead, with the
   * timeout as its reason, and every other call on it fails with that.
   */
  #abandon(error: RabbitError): void {
    if (this.#closeError !== null || this.#closed) return;
    this.#closeError = error;
    for (const waiter of this.#waiters.splice(0)) {
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      waiter.reject(error);
    }
    for (const [, confirmation] of this.#outstanding) confirmation.reject(error);
    this.#outstanding.clear();
    void this.#connection.closeChannel(this.#number);
  }

  #take(name: string): ((fulfilment: Fulfilment) => void) | null {
    for (let at = 0; at < this.#waiters.length; at++) {
      const waiter = this.#waiters[at]!;
      if (!waiter.names.includes(name)) continue;
      this.#waiters.splice(at, 1);
      if (waiter.timer !== null) clearTimeout(waiter.timer);
      return waiter.resolve;
    }
    return null;
  }

  async #send(name: string, args: Record<string, MethodArg>): Promise<void> {
    this.#requireOpen();
    await this.#connection.send(this.#number, name, args);
  }

  #requireOpen(): void {
    if (this.#number === 0) {
      throw new RabbitError("the channel has not been opened on its connection", {
        code: RabbitErrorCode.Channel,
      });
    }
    if (this.#closeError !== null) throw this.#closeError;
    if (this.#closed) {
      throw new RabbitError("the channel is closed", { code: RabbitErrorCode.Closed });
    }
  }

  // -- turning frames into messages and answers -----------------------------

  #complete(): void {
    const method = this.#method;
    const header = this.#header;
    const body = this.#join();
    this.#method = null;
    this.#header = null;
    this.#chunks = [];
    this.#bodySize = 0;
    this.#received = 0;
    if (method === null || header === null) return;
    this.#dispatch({ method, message: messageOf(method, header, body) });
  }

  #join(): Uint8Array {
    if (this.#chunks.length === 0) return new Uint8Array(0);
    if (this.#chunks.length === 1) return this.#chunks[0]!;
    const body = new Uint8Array(this.#received);
    let at = 0;
    for (const chunk of this.#chunks) {
      body.set(chunk, at);
      at += chunk.length;
    }
    return body;
  }

  #dispatch(fulfilment: Fulfilment): void {
    const waiter = this.#take(fulfilment.method.name);
    if (waiter !== null) {
      waiter(fulfilment);
      return;
    }
    switch (fulfilment.method.name) {
      case "basic.deliver":
        if (fulfilment.message !== null) this.onDelivery?.(fulfilment.message);
        return;
      case "basic.return":
        if (fulfilment.message !== null) this.onReturn?.(fulfilment.message);
        return;
      case "basic.cancel":
        // A broker-initiated cancellation (`consumer_cancel_notify`): it is
        // waiting for the acknowledgement, then the consumer layer hears about
        // it. One that raced our own `basic.cancel` instead went to a waiter.
        if (this.#number !== 0 && fulfilment.method.args.noWait !== true) {
          void this.#connection
            .send(this.#number, "basic.cancel-ok", {
              consumerTag: String(fulfilment.method.args.consumerTag ?? ""),
            })
            .catch(() => {});
        }
        this.onCancel?.(String(fulfilment.method.args.consumerTag ?? ""));
        return;
      case "basic.ack":
      case "basic.nack":
        this.#settle(fulfilment.method);
        return;
      case "channel.flow":
        // Flow control is only meaningful with a queue of unpublished messages
        // to hold back, which this layer does not keep; the protocol still
        // wants an answer, so it gets one. Before `attach` there is no channel
        // number to answer on, and such a frame cannot arrive then.
        if (this.#number !== 0) {
          void this.#connection
            .send(this.#number, "channel.flow-ok", {
              active: fulfilment.method.args.active ?? false,
            })
            .catch(() => {});
        }
        return;
      default:
        return;
    }
  }

  #track(sequence: bigint): Promise<void> {
    let settle!: { resolve: () => void; reject: (error: RabbitError) => void };
    const promise = new Promise<void>((resolve, reject) => {
      settle = { resolve, reject };
    });
    this.#outstanding.set(sequence, { promise, ...settle });
    return promise;
  }

  // A publisher confirm: `multiple` covers every tag up to and including this
  // one, which is how a run of acks is collapsed onto the last of them.
  #settle(method: DecodedMethod): void {
    const tag = asTag(method.args.deliveryTag ?? 0n);
    const multiple = method.args.multiple === true;
    const ack = method.name === "basic.ack";
    // `#outstanding` is in sequence order (a Map keeps insertion order and
    // sequences only grow), so nothing past `tag` can match: stopping there
    // keeps a steady stream of single acks from scanning every message still
    // in flight. Deleting the entry being visited is safe in a Map iteration,
    // so the map is walked in place rather than copied first.
    for (const [sequence, confirmation] of this.#outstanding) {
      if (sequence > tag) break;
      if (sequence !== tag && !multiple) continue;
      this.#outstanding.delete(sequence);
      if (ack) confirmation.resolve();
      else confirmation.reject(nacked(tag, method.args.requeue === true));
    }
  }
}

// -- options that flatten onto the properties --------------------------------

/** What {@link Channel.publish} takes: the message's properties, and `mandatory`. */
export interface PublishOptions extends BasicProperties {
  /**
   * Ask the broker to return the message — via {@link Channel.onReturn} —
   * rather than silently drop it when no queue is bound to accept it.
   */
  mandatory?: boolean;
}

export interface GetOptions {
  /** Treat the message as acknowledged the moment it is delivered. */
  noAck?: boolean;
}

export interface AckOptions {
  /** Acknowledge every unacknowledged delivery up to and including this one. */
  multiple?: boolean;
}

export interface NackOptions extends AckOptions {
  /** Put the message back on the queue. Defaults to true. */
  requeue?: boolean;
}

export interface RejectOptions {
  /** Put the message back on the queue. Defaults to true. */
  requeue?: boolean;
}

// -- helpers -----------------------------------------------------------------

function messageOf(method: DecodedMethod, header: ContentHeader, body: Uint8Array): Message {
  const args = method.args;
  const message: Message = {
    deliveryTag: args.deliveryTag === undefined ? 0n : asTag(args.deliveryTag),
    redelivered: args.redelivered === true,
    exchange: String(args.exchange ?? ""),
    routingKey: String(args.routingKey ?? ""),
    properties: header.properties,
    body,
  };
  if (typeof args.consumerTag === "string") message.consumerTag = args.consumerTag;
  if (args.messageCount !== undefined) message.messageCount = asNumber(args.messageCount);
  if (method.name === "basic.return") {
    message.returned = {
      replyCode: asNumber(args.replyCode),
      replyText: String(args.replyText ?? ""),
    };
  }
  return message;
}

function nacked(tag: bigint, requeue: boolean): RabbitError {
  return new RabbitError(
    `the broker rejected the message (delivery tag ${tag}${requeue ? ", requeued" : ""})`,
    { code: RabbitErrorCode.Nack, soft: true },
  );
}

function asTag(value: MethodArg | bigint | number): bigint {
  if (typeof value === "bigint") return value;
  if (typeof value === "number" && Number.isSafeInteger(value) && value >= 0) return BigInt(value);
  throw new RabbitError(`a delivery tag must be a non-negative integer: ${String(value)}`, {
    code: RabbitErrorCode.Channel,
  });
}

function asNumber(value: MethodArg | undefined): number {
  if (typeof value === "number") return value;
  if (typeof value === "bigint") return Number(value);
  throw new RabbitError(`expected a number, found ${typeof value}`, {
    code: RabbitErrorCode.Protocol,
  });
}
