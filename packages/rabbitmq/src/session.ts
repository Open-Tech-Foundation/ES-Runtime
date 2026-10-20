/**
 * Recovery: a connection that comes back, and channels and subscriptions that
 * carry on across it (DECISIONS D177).
 *
 * A {@link Connection} is a socket, latched when it fails, and nothing below
 * this layer pretends otherwise. A {@link Session} is the policy over a run of
 * connections: when one is lost it dials again — with backoff, round-robin over
 * the URLs it was given — and then re-runs what the application asked for, in
 * order. Each {@link RecoveringChannel} is reopened, re-selects confirms if it
 * asked for them, runs its own `setup`, and re-consumes each of its
 * subscriptions.
 *
 * ```js
 * const session = await openSession("amqp://localhost/");
 * const jobs = await session.channel({
 *   confirm: true,
 *   setup: (channel) => channel.queueDeclare("jobs", { durable: true }),
 * });
 * await jobs.publish("", "jobs", "hello"); // waits out a reconnect, within its timeout
 * for await (const delivery of jobs.subscribe("jobs", { prefetch: 16 })) {
 *   await handle(delivery);
 *   await delivery.ack();
 * }
 * ```
 *
 * What it will not do is the part that cannot be done honestly:
 *
 * - **No replay.** Topology is whatever `setup` declares, run again; nothing
 *   is restored from a cache this layer kept behind the application's back.
 * - **No republish.** A publish whose confirm was outstanding when the socket
 *   went has an unknown fate, so it rejects; sending it again could deliver it
 *   twice.
 * - **No stale acknowledgements.** A delivery tag belongs to its channel, and
 *   the broker requeues what was unacknowledged when that channel died. So a
 *   delivery from before the drop settles without sending, and one buffered
 *   but never read is discarded; the broker redelivers both.
 */

import type { Channel, Delivery, PublishOptions } from "./channel.js";
import type { Connection } from "./connection.js";
import { type ConsumeOptions, type Subscription, subscribe } from "./consumer.js";
import { RabbitError, RabbitErrorCode } from "./errors.js";
import { type OpenSettings, openConnection, parseAmqpUrl } from "./pool.js";
import type { NetTransportOptions, Transport } from "./transport.js";

/** How long anything waits when nothing else says, in ms. */
const DEFAULT_TIMEOUT = 30_000;
const DEFAULT_INITIAL_DELAY = 500;
const DEFAULT_MAX_DELAY = 30_000;

export interface ReconnectOptions {
  /** The first retry's delay, in ms. Default 500. */
  initialDelay?: number;
  /** The longest delay between retries, in ms. Default 30 000. */
  maxDelay?: number;
}

export interface SessionOptions extends OpenSettings {
  /** Retry timing: exponential backoff with jitter between these bounds. */
  reconnect?: ReconnectOptions;
  /** How to open the socket. Defaults to `netTransport`; a test scripts one. */
  dial?: (options: NetTransportOptions) => Promise<Transport>;
}

export interface RecoveringChannelOptions {
  /**
   * Runs on every (re)open, before the channel is used or its subscriptions
   * are re-consumed — where queues, exchanges, bindings and `qos` are
   * declared. It runs again after every reconnect, so it must be safe to
   * repeat, as a declaration is.
   */
  setup?: (channel: Channel) => void | Promise<void>;
  /** Put the channel in publisher-confirm mode on every open. */
  confirm?: boolean;
  /** How long any one method on the channel waits, in ms. Default 30 000. */
  timeout?: number;
}

/** Errors that will not get better by dialling again. */
function permanent(error: unknown): boolean {
  return (
    error instanceof RabbitError &&
    (error.code === RabbitErrorCode.Auth ||
      error.code === RabbitErrorCode.Handshake ||
      error.code === RabbitErrorCode.Unsupported)
  );
}

/** A promise with its settlers, re-armed whenever the thing it waits for goes. */
class Signal<T> {
  promise!: Promise<T>;
  #resolve!: (value: T) => void;
  #reject!: (error: Error) => void;
  #settled = false;

  constructor() {
    this.reset();
  }

  reset(): void {
    if (!this.#settled && this.promise !== undefined) return;
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

/** `promise`, or a timeout error after `ms` (`0` or less: no limit). */
function within<T>(promise: Promise<T>, ms: number, what: () => string): Promise<T> {
  if (ms <= 0) return promise;
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new RabbitError(what(), { code: RabbitErrorCode.Timeout })),
      ms,
    );
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

const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Opens a session: dials the first reachable URL and resolves once connected.
 *
 * A transient failure — a refused or lost socket, a timeout — is retried with
 * backoff until `options.timeout` (30 000 ms) has passed. Credentials refused,
 * a version that cannot be agreed, or an unsupported URL reject at once.
 */
export async function openSession(
  urls: string | readonly string[],
  options: SessionOptions = {},
): Promise<Session> {
  const session = new Session(typeof urls === "string" ? [urls] : [...urls], options);
  await SESSIONS.get(session)!.start();
  return session;
}

// Module-private hooks between the three classes. They live here rather than
// as methods so that nothing a caller can reach — or a declaration file
// spells — exposes recovery's internals.
interface SessionHooks {
  start(): Promise<void>;
  forget(channel: RecoveringChannel): void;
}
interface ChannelHooks {
  reopen(connection: Connection, first: boolean): Promise<void>;
  lost(): void;
}
interface SubscriptionHooks {
  resubscribe(channel: Channel): Promise<void>;
}
const SESSIONS = new WeakMap<Session, SessionHooks>();
const CHANNELS = new WeakMap<RecoveringChannel, ChannelHooks>();
const SUBSCRIPTIONS = new WeakMap<RecoveringSubscription, SubscriptionHooks>();

/**
 * One broker conversation that outlives its connections.
 *
 * It holds at most one {@link Connection}; when that one is lost it dials a
 * replacement and reopens every {@link RecoveringChannel} on it.
 */
export class Session {
  readonly #urls: string[];
  readonly #options: SessionOptions;
  readonly #initialDelay: number;
  readonly #maxDelay: number;
  readonly #timeout: number;

  #next = 0;
  #attempt = 0;
  #connection: Connection | null = null;
  #ready = new Signal<Connection>();
  #channels = new Set<RecoveringChannel>();
  #timer: ReturnType<typeof setTimeout> | null = null;
  #closed = false;

  /** A connection is open — the first, or a replacement. */
  onConnect: ((connection: Connection) => void) | null = null;
  /** The connection was lost; recovery has begun. */
  onDisconnect: ((error: RabbitError) => void) | null = null;
  /** A reconnect attempt, or a channel's reopen or setup, failed; it will be retried. */
  onError: ((error: Error) => void) | null = null;

  constructor(urls: string[], options: SessionOptions) {
    if (urls.length === 0) {
      throw new RabbitError("a session needs at least one URL", {
        code: RabbitErrorCode.Connection,
      });
    }
    // A URL that cannot be read will not read better on a retry: refused here,
    // before the first dial, rather than retried until the timeout.
    for (const url of urls) parseAmqpUrl(url);
    this.#urls = urls;
    this.#options = options;
    this.#initialDelay = options.reconnect?.initialDelay ?? DEFAULT_INITIAL_DELAY;
    this.#maxDelay = options.reconnect?.maxDelay ?? DEFAULT_MAX_DELAY;
    this.#timeout = options.timeout ?? DEFAULT_TIMEOUT;
    SESSIONS.set(this, {
      start: () => this.#start(),
      forget: (channel) => {
        this.#channels.delete(channel);
      },
    });
  }

  /** The live connection, or `null` while reconnecting. */
  get connection(): Connection | null {
    return this.#connection;
  }

  /** Whether a connection is open right now. */
  get connected(): boolean {
    return this.#connection !== null;
  }

  /** Whether {@link close} has been called. */
  get closed(): boolean {
    return this.#closed;
  }

  /** Resolves with the live connection — at once, or after the reconnect. */
  whenConnected(timeout: number = this.#timeout): Promise<Connection> {
    if (this.#closed) return Promise.reject(this.#closedError());
    if (this.#connection !== null) return Promise.resolve(this.#connection);
    return within(this.#ready.promise, timeout, () => `no connection within ${timeout}ms`);
  }

  /**
   * Opens a channel that is reopened, and set up again, after every reconnect.
   * Resolves once its first open and `setup` have succeeded.
   */
  async channel(options: RecoveringChannelOptions = {}): Promise<RecoveringChannel> {
    if (this.#closed) throw this.#closedError();
    const channel = new RecoveringChannel(this, options, this.#initialDelay, this.#maxDelay);
    this.#channels.add(channel);
    const connection = await this.whenConnected();
    try {
      await CHANNELS.get(channel)!.reopen(connection, true);
    } catch (e) {
      this.#channels.delete(channel);
      throw e;
    }
    return channel;
  }

  /** Stops recovering, closes every channel and the connection. */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    if (this.#timer !== null) clearTimeout(this.#timer);
    this.#timer = null;
    this.#ready.reject(this.#closedError());
    await Promise.all([...this.#channels].map((channel) => channel.close().catch(() => {})));
    const connection = this.#connection;
    this.#connection = null;
    await connection?.close().catch(() => {});
  }

  // -- the machinery --------------------------------------------------------

  async #start(): Promise<void> {
    const deadline = this.#timeout > 0 ? Date.now() + this.#timeout : Number.POSITIVE_INFINITY;
    for (;;) {
      try {
        this.#adopt(await this.#dial());
        return;
      } catch (e) {
        if (permanent(e) || Date.now() + this.#delay() > deadline) {
          this.#closed = true;
          throw e;
        }
        this.onError?.(e instanceof Error ? e : new Error(String(e)));
        await sleep(this.#delay());
        this.#attempt++;
      }
    }
  }

  #dial(): Promise<Connection> {
    const url = this.#urls[this.#next % this.#urls.length]!;
    this.#next++;
    const { reconnect: _reconnect, dial, ...settings } = this.#options;
    return openConnection(url, settings, dial);
  }

  #adopt(connection: Connection): void {
    if (this.#closed) {
      void connection.close().catch(() => {});
      return;
    }
    this.#attempt = 0;
    this.#connection = connection;
    connection.onClosed = (error) => this.#lost(connection, error);
    this.#ready.resolve(connection);
    this.onConnect?.(connection);
    for (const channel of this.#channels) {
      void CHANNELS.get(channel)!
        .reopen(connection, false)
        .catch(() => {});
    }
  }

  #lost(connection: Connection, error: RabbitError | null): void {
    if (this.#connection !== connection) return;
    this.#connection = null;
    // `null` is a deliberate close, which only `close()` makes.
    if (this.#closed || error === null) return;
    this.#ready.reset();
    for (const channel of this.#channels) CHANNELS.get(channel)!.lost();
    this.onDisconnect?.(error);
    this.#schedule();
  }

  #schedule(): void {
    if (this.#closed || this.#timer !== null) return;
    this.#timer = setTimeout(() => {
      this.#timer = null;
      void this.#dial().then(
        (connection) => this.#adopt(connection),
        (e: unknown) => {
          this.onError?.(e instanceof Error ? e : new Error(String(e)));
          this.#attempt++;
          this.#schedule();
        },
      );
    }, this.#delay());
  }

  // Exponential backoff with "equal jitter": half the delay is fixed and half
  // random, so a restarted cluster's clients do not all return at once.
  #delay(): number {
    const ceiling = Math.min(this.#maxDelay, this.#initialDelay * 2 ** Math.min(this.#attempt, 30));
    return ceiling / 2 + Math.random() * (ceiling / 2);
  }

  #closedError(): RabbitError {
    return new RabbitError("the session is closed", { code: RabbitErrorCode.Closed });
  }
}

/**
 * A channel that is reopened, and set up again, whenever its connection or the
 * channel itself is lost.
 *
 * Publish and subscribe through it; for anything else, {@link current} is the
 * live {@link Channel} (or `null` while it is being recovered).
 */
export class RecoveringChannel {
  readonly #session: Session;
  readonly #options: RecoveringChannelOptions;
  readonly #timeout: number;
  readonly #initialDelay: number;
  readonly #maxDelay: number;

  #channel: Channel | null = null;
  #ready = new Signal<Channel>();
  #subscriptions = new Set<RecoveringSubscription>();
  #attempt = 0;
  #closed = false;

  constructor(
    session: Session,
    options: RecoveringChannelOptions,
    initialDelay: number,
    maxDelay: number,
  ) {
    this.#session = session;
    this.#options = options;
    this.#timeout = options.timeout ?? DEFAULT_TIMEOUT;
    this.#initialDelay = initialDelay;
    this.#maxDelay = maxDelay;
    CHANNELS.set(this, {
      reopen: (connection, first) => this.#reopen(connection, first),
      lost: () => this.#lost(),
    });
  }

  /** The live channel, or `null` while it is being recovered. */
  get current(): Channel | null {
    return this.#channel;
  }

  /** Resolves with the live channel — at once, or once it is recovered. */
  whenReady(timeout: number = this.#timeout): Promise<Channel> {
    if (this.#closed) return Promise.reject(closedChannel());
    if (this.#channel !== null) return Promise.resolve(this.#channel);
    return within(this.#ready.promise, timeout, () => `no channel within ${timeout}ms`);
  }

  /**
   * Publishes once a live channel exists — waiting out a reconnect — within
   * `options.timeout` for the whole of it. Never republished: if the channel
   * is lost before the publish is confirmed (or, without confirms, written),
   * it rejects, because the broker may or may not have it.
   */
  async publish(
    exchange: string,
    routingKey: string,
    body: string | Uint8Array,
    options: PublishOptions = {},
  ): Promise<void> {
    const timeout = options.timeout ?? this.#timeout;
    const started = Date.now();
    const channel = await this.whenReady(timeout);
    const left = timeout > 0 ? Math.max(1, timeout - (Date.now() - started)) : 0;
    await channel.publish(exchange, routingKey, body, { ...options, timeout: left });
  }

  /**
   * Subscribes to `queue`, and re-consumes after every recovery with the same
   * options. `queue` may be a function, read on every (re)subscribe — for a
   * server-named queue that `setup` declares afresh each time.
   */
  subscribe(queue: string | (() => string), options: ConsumeOptions = {}): RecoveringSubscription {
    if (this.#closed) throw closedChannel();
    const subscription = new RecoveringSubscription(this, queue, options);
    this.#subscriptions.add(subscription);
    if (this.#channel !== null) {
      void SUBSCRIPTIONS.get(subscription)!.resubscribe(this.#channel);
    }
    return subscription;
  }

  /** Stops recovering it: cancels its subscriptions and closes the channel. */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    SESSIONS.get(this.#session)!.forget(this);
    this.#ready.reject(closedChannel());
    await Promise.all([...this.#subscriptions].map((one) => one.cancel().catch(() => {})));
    const channel = this.#channel;
    this.#channel = null;
    await channel?.close().catch(() => {});
  }

  /** @internal Unsubscribing, from the subscription itself. */
  forget(subscription: RecoveringSubscription): void {
    this.#subscriptions.delete(subscription);
  }

  // -- recovery ---------------------------------------------------------------

  /**
   * Opens the channel on `connection`, re-selects confirms, runs `setup`, and
   * re-consumes the subscriptions. On the first open a failure is the caller's;
   * on a recovery it is reported and retried with backoff while the
   * connection lasts (a lost connection brings its own retry).
   */
  async #reopen(connection: Connection, first: boolean): Promise<void> {
    if (this.#closed) return;
    let channel: Channel | null = null;
    try {
      channel = await connection.channel({ timeout: this.#timeout });
      if (this.#options.confirm === true) await channel.confirmSelect();
      await this.#options.setup?.(channel);
    } catch (e) {
      await channel?.close().catch(() => {});
      if (first) throw e;
      this.#session.onError?.(e instanceof Error ? e : new Error(String(e)));
      this.#retry(connection);
      return;
    }
    if (this.#closed) {
      await channel.close().catch(() => {});
      return;
    }
    this.#attempt = 0;
    this.#channel = channel;
    const opened = channel;
    void opened.whenClosed().then((error) => this.#channelLost(opened, connection, error));
    this.#ready.resolve(channel);
    for (const subscription of this.#subscriptions) {
      void SUBSCRIPTIONS.get(subscription)!.resubscribe(channel);
    }
  }

  /** The session lost the connection: the channel went with it. */
  #lost(): void {
    if (this.#channel === null) return;
    this.#channel = null;
    this.#ready.reset();
  }

  // The broker closed this channel on a connection that is still up — a
  // refused declaration, a bad acknowledgement. It is reopened on the same
  // connection; a lost connection is the session's to recover instead.
  #channelLost(channel: Channel, connection: Connection, error: RabbitError | null): void {
    if (this.#channel !== channel) return;
    this.#channel = null;
    this.#ready.reset();
    if (this.#closed || error === null) return;
    if (connection.usable && this.#session.connection === connection) {
      this.#session.onError?.(error);
      this.#retry(connection);
    }
  }

  #retry(connection: Connection): void {
    const ceiling = Math.min(this.#maxDelay, this.#initialDelay * 2 ** Math.min(this.#attempt, 30));
    this.#attempt++;
    const delay = ceiling / 2 + Math.random() * (ceiling / 2);
    setTimeout(() => {
      if (this.#closed || this.#channel !== null) return;
      if (!connection.usable || this.#session.connection !== connection) return;
      void this.#reopen(connection, false).catch(() => {});
    }, delay);
  }
}

/**
 * A subscription that carries on across recoveries: the same async iterable
 * of deliveries, re-consumed on each new channel.
 *
 * It ends only when it is cancelled, when the broker cancels the consumer (its
 * queue deleted, say), or when its channel or session is closed.
 */
export class RecoveringSubscription implements AsyncIterable<Delivery> {
  readonly #owner: RecoveringChannel;
  readonly #queue: string | (() => string);
  readonly #options: ConsumeOptions;

  #current: Subscription | null = null;
  #iterator: AsyncIterator<Delivery> | null = null;
  #ready = new Signal<void>();
  #done = false;
  #ending: Error | null = null;

  constructor(owner: RecoveringChannel, queue: string | (() => string), options: ConsumeOptions) {
    this.#owner = owner;
    this.#queue = queue;
    this.#options = options;
    SUBSCRIPTIONS.set(this, { resubscribe: (channel) => this.#resubscribe(channel) });
  }

  /** Whether it has ended. */
  get closed(): boolean {
    return this.#done;
  }

  /** The live subscription underneath, or `null` while recovering. */
  get current(): Subscription | null {
    return this.#current;
  }

  /** Cancels the consumer and ends the iteration. */
  async cancel(): Promise<void> {
    if (this.#done) return;
    this.#done = true;
    this.#owner.forget(this);
    this.#ready.resolve();
    const current = this.#current;
    this.#current = null;
    this.#iterator = null;
    await current?.cancel().catch(() => {});
  }

  [Symbol.asyncIterator](): AsyncIterableIterator<Delivery> {
    return {
      next: () => this.#next(),
      return: async () => {
        await this.cancel();
        return { value: undefined, done: true };
      },
      [Symbol.asyncIterator]() {
        return this;
      },
    };
  }

  async #resubscribe(channel: Channel): Promise<void> {
    if (this.#done) return;
    const queue = typeof this.#queue === "function" ? this.#queue() : this.#queue;
    let subscription: Subscription;
    try {
      subscription = await subscribe(channel, queue, this.#options);
    } catch {
      // The channel will have been closed by the refusal, which reopens it
      // and resubscribes again; nothing more to do here.
      return;
    }
    if (this.#done || !channel.usable) {
      await subscription.cancel().catch(() => {});
      return;
    }
    this.#current = subscription;
    this.#iterator = subscription[Symbol.asyncIterator]();
    this.#ready.resolve();
  }

  async #next(): Promise<IteratorResult<Delivery>> {
    for (;;) {
      if (this.#done) {
        if (this.#ending !== null) {
          const error = this.#ending;
          this.#ending = null;
          throw error;
        }
        return { value: undefined, done: true };
      }
      const iterator = this.#iterator;
      const current = this.#current;
      if (iterator === null || current === null) {
        await this.#ready.promise;
        continue;
      }
      let result: IteratorResult<Delivery>;
      try {
        result = await iterator.next();
      } catch {
        // The channel died under it. Recovery resubscribes; wait for that.
        this.#detach(current);
        continue;
      }
      if (!result.done) {
        // Read out of the buffer of a channel that has since died: the broker
        // requeued it and will deliver it again, so it is not handed over.
        if (!current.closed || current.cancellation === null) {
          return { value: staleSafe(result.value, current), done: false };
        }
        if (current.cancellation.error !== null) continue;
        return { value: staleSafe(result.value, current), done: false };
      }
      // The iteration ended without an error: cancelled by the broker, or by
      // us. Either way this subscription is over.
      if (current.cancellation?.byBroker === true || this.#current === current) {
        this.#done = true;
        this.#owner.forget(this);
        continue;
      }
      this.#detach(current);
    }
  }

  #detach(current: Subscription): void {
    if (this.#current !== current) return;
    this.#current = null;
    this.#iterator = null;
    this.#ready.reset();
  }
}

// A delivery whose channel is gone has a tag no channel knows any more; the
// broker requeued it when that channel died. Settling it then sends nothing.
function staleSafe(delivery: Delivery, subscription: Subscription): Delivery {
  const guard =
    <T extends unknown[]>(settle: (...args: T) => Promise<void>) =>
    async (...args: T): Promise<void> => {
      if (subscription.cancellation?.error != null) return;
      try {
        await settle(...args);
      } catch (e) {
        if (subscription.cancellation?.error != null) return;
        throw e;
      }
    };
  return {
    ...delivery,
    ack: guard(delivery.ack),
    nack: guard(delivery.nack),
    reject: guard(delivery.reject),
  };
}

function closedChannel(): RabbitError {
  return new RabbitError("the recovering channel is closed", { code: RabbitErrorCode.Closed });
}
