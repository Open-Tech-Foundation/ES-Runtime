/**
 * Consuming: a queue's messages, as an async iterable.
 *
 * A channel already turns frames into messages; this layer decides what a
 * *subscription* is. A subscription starts a broker consumer, hands each
 * delivery to the code that asked for it, and ends — when it is cancelled, when
 * the broker cancels it, or when the channel dies.
 *
 * ```js
 * const work = await subscribe(channel, "jobs", { prefetch: 10 }); // in flight at once
 * for await (const delivery of work) {
 *   await handle(delivery.body);
 *   await delivery.ack();
 * }
 * ```
 *
 * Three decisions are worth naming, because a consumer layer is mostly these:
 *
 * - **The consumer tag is chosen before the first delivery can arrive.** A
 *   delivery names the consumer it was for, so if the tag were only learned when
 *   the broker answers the subscription, a delivery that raced the answer could
 *   not be routed. The tag is therefore ours (generated if the caller did not
 *   pick one) and is sent in `basic.consume`; nothing arrives that cannot be
 *   matched.
 * - **Acknowledgement is manual by default.** The iterator yields the message
 *   and waits; `ack`, `nack`, and `reject` on the delivery are how it is
 *   settled. A consumer that acknowledges before the work succeeds is a
 *   decision for the application, not a default here.
 * - **Flow control is `basic.qos`, and it is never left unlimited by
 *   default.** The prefetch is what bounds unacknowledged deliveries — and so
 *   this layer's buffer — and RabbitMQ's own default is no limit, which pulls a
 *   whole queue into memory. A subscription therefore sets
 *   {@link DEFAULT_PREFETCH} when neither it nor the channel names one.
 *
 * Ending the iteration — a `break`, or {@link Subscription.cancel} — cancels
 * the consumer with the broker. A broken connection ends it too, with the error
 * that broke it.
 */

import type { Channel, Delivery, Message } from "./channel.js";
import { RabbitError, RabbitErrorCode } from "./errors.js";
import type { FieldTable } from "./protocol/fields.js";

/**
 * The prefetch a subscription takes when its channel has none. RabbitMQ's own
 * guidance is that "values in the 100 through 300 range usually offer optimal
 * throughput", and Spring AMQP defaults to 250. Unlimited — the broker's default
 * — pulls a whole queue into the subscription's buffer.
 */
export const DEFAULT_PREFETCH = 250;

export interface ConsumeOptions {
  /** A tag of your choosing; one is generated otherwise. Unique per channel. */
  consumerTag?: string;
  /**
   * How many deliveries may be unacknowledged at once, for this consumer. When
   * absent, the prefetch already set on the channel with `channel.qos()` is
   * used, or {@link DEFAULT_PREFETCH} if none was. `0` asks for no limit.
   * Ignored with `noAck`, where nothing is held for acknowledgement.
   */
  prefetch?: number;
  /**
   * Handle the message as acknowledged the moment it is delivered. Off by
   * default, which is what makes a crash lose nothing: the broker keeps the
   * message until you `ack` it.
   */
  noAck?: boolean;
  /** Only this consumer may use the queue, and it is deleted when it ends. */
  exclusive?: boolean;
  /** Consumer arguments, such as `x-priority` or a dead-letter policy. */
  arguments?: FieldTable;
}

/** How the broker or the channel ended a subscription. */
export interface Cancellation {
  /** The broker cancelled the consumer (`consumer_cancel_notify`). */
  byBroker: boolean;
  /** What ended it, when it was not a deliberate cancellation. */
  error: Error | null;
}

let nextTag = 1;

// One registry per channel: the consumer tags in flight, and whether the
// channel's delivery hooks have been claimed by this layer yet.
interface Registry {
  consumers: Map<string, Subscription>;
  installed: boolean;
  // Subscribing is `basic.qos` then `basic.consume`, and RabbitMQ applies the
  // prefetch in force when the consume arrives. Two subscriptions started at
  // once on one channel would interleave the pairs and swap limits, so each
  // takes its turn on this chain.
  starting: Promise<void>;
}

const REGISTRIES = new WeakMap<Channel, Registry>();

function registryFor(channel: Channel): Registry {
  let registry = REGISTRIES.get(channel);
  if (registry === undefined) {
    registry = { consumers: new Map(), installed: false, starting: Promise.resolve() };
    REGISTRIES.set(channel, registry);
  }
  return registry;
}

/**
 * Subscribes to `queue` and answers with the subscription.
 *
 * The prefetch is `options.prefetch`, else the channel's own from
 * `channel.qos()`, else {@link DEFAULT_PREFETCH}; it is what bounds how many
 * unacknowledged messages the broker sends, and so how many this subscription
 * buffers.
 */
export async function subscribe(
  channel: Channel,
  queue: string,
  options: ConsumeOptions = {},
): Promise<Subscription> {
  const registry = registryFor(channel);
  install(channel, registry);
  const consumerTag = options.consumerTag ?? `esrun-${nextTag++}`;
  const noAck = options.noAck ?? false;
  const subscription = new Subscription(channel, registry, queue, consumerTag, !noAck);
  registry.consumers.set(consumerTag, subscription);
  // A broken channel ends every subscription on it; the reason is the channel's.
  void channel.whenClosed().then((error) => subscription.finish(error, false));

  const start = async (): Promise<void> => {
    if (!noAck) {
      const prefetch = options.prefetch ?? channel.consumerPrefetch ?? DEFAULT_PREFETCH;
      if (prefetch !== channel.consumerPrefetch) await channel.qos({ prefetchCount: prefetch });
    }
    await channel.consume(queue, {
      consumerTag,
      noAck,
      exclusive: options.exclusive ?? false,
      ...(options.arguments === undefined ? {} : { arguments: options.arguments }),
    });
  };
  const started = registry.starting.then(start, start);
  registry.starting = started.then(
    () => {},
    () => {},
  );
  try {
    await started;
  } catch (e) {
    await subscription.cancel().catch(() => {});
    throw e;
  }
  return subscription;
}

// The channel hands deliveries to one hook, so this layer claims it and routes
// by consumer tag. It is claimed once, however many subscriptions follow.
function install(channel: Channel, registry: Registry): void {
  if (registry.installed) return;
  registry.installed = true;
  channel.onDelivery = (message) => {
    const tag = message.consumerTag;
    if (tag === undefined) return;
    registry.consumers.get(tag)?.offer(message);
  };
  channel.onCancel = (tag) => {
    registry.consumers.get(tag)?.finish(null, true);
  };
}

/**
 * One queue's messages, pulled one at a time.
 *
 * It is an async iterable of deliveries; breaking out of a `for await` cancels
 * the consumer, and so does asking for {@link cancel}.
 */
export class Subscription implements AsyncIterable<Delivery> {
  readonly #channel: Channel;
  readonly #registry: Registry;
  readonly #queue: string;
  readonly #tag: string;
  readonly #manual: boolean;

  #buffer: Delivery[] = [];
  #waiting: Array<{
    resolve: (result: IteratorResult<Delivery>) => void;
    reject: (error: Error) => void;
  }> = [];
  #done = false;
  // Set when this side cancelled, as opposed to the broker or a dead channel:
  // only then are deliveries still on their way ours to hand back.
  #cancelled = false;
  #error: Error | null = null;
  #cancellation: Cancellation | null = null;

  constructor(channel: Channel, registry: Registry, queue: string, tag: string, manual: boolean) {
    this.#channel = channel;
    this.#registry = registry;
    this.#queue = queue;
    this.#tag = tag;
    this.#manual = manual;
  }

  /** The consumer tag the broker knows this subscription by. */
  get consumerTag(): string {
    return this.#tag;
  }

  /** The queue it is consuming. */
  get queue(): string {
    return this.#queue;
  }

  /** Whether it has ended. */
  get closed(): boolean {
    return this.#done;
  }

  /** How it ended, once it has; `null` while it is still running. */
  get cancellation(): Cancellation | null {
    return this.#cancellation;
  }

  /** Routes a delivery to the iterator, or buffers it if nobody is waiting. */
  offer(message: Message): void {
    if (this.#done) {
      // A delivery that raced our `basic.cancel`. Nobody will read it, and
      // the broker does not take it back on cancel, so it is handed back now.
      if (this.#cancelled) this.#requeue([message]);
      return;
    }
    const delivery = deliveryOf(this.#channel, message, this.#manual);
    const waiter = this.#waiting.shift();
    if (waiter !== undefined) waiter.resolve({ value: delivery, done: false });
    else this.#buffer.push(delivery);
  }

  /** Ends the subscription, with an error, or by the broker's cancellation. */
  finish(error: Error | null, byBroker: boolean): void {
    if (this.#done) return;
    this.#done = true;
    this.#error = error;
    this.#cancellation = { byBroker, error };
    // A cancel of our own keeps the tag routed until the broker confirms it,
    // so a delivery racing `basic.cancel` still reaches `offer` and is handed
    // back instead of vanishing (see `cancel`).
    if (!this.#cancelled) this.#registry.consumers.delete(this.#tag);
    for (const waiter of this.#waiting.splice(0)) {
      if (error !== null) waiter.reject(error);
      else waiter.resolve({ value: undefined, done: true });
    }
  }

  /** Cancels the consumer with the broker and ends the iteration. */
  async cancel(): Promise<void> {
    if (this.#done) return;
    // Marked done first, so nothing more is yielded and a racing
    // `basic.deliver` is handed straight back (see `offer`).
    this.#cancelled = true;
    const unread = this.#buffer.splice(0);
    this.finish(null, false);
    try {
      await this.#channel.cancelConsumer(this.#tag);
    } catch {
      // The channel is gone, which is the end the caller asked for anyway.
    }
    // After `cancel-ok` nothing more is delivered under this tag.
    if (this.#registry.consumers.get(this.#tag) === this) {
      this.#registry.consumers.delete(this.#tag);
    }
    // `basic.cancel` "does not affect already delivered messages" (AMQP 0-9-1):
    // a delivery that arrived but was never read stays unacknowledged, held
    // for this channel until it closes. On a long-lived channel that is
    // forever, so each is returned to its queue rather than dropped.
    this.#requeue(unread);
  }

  // Hands deliveries back to the broker for redelivery. Only a manual-ack
  // subscription has anything to hand back; with `noAck` the broker already
  // forgot them. A channel that died meanwhile requeues them itself.
  #requeue(messages: readonly Message[]): void {
    if (!this.#manual) return;
    for (const message of messages) {
      void this.#channel.reject(message.deliveryTag, { requeue: true }).catch(() => {});
    }
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

  #next(): Promise<IteratorResult<Delivery>> {
    const buffered = this.#buffer.shift();
    if (buffered !== undefined) return Promise.resolve({ value: buffered, done: false });
    if (this.#error !== null) return Promise.reject(this.#error);
    if (this.#done) return Promise.resolve({ value: undefined, done: true });
    return new Promise((resolve, reject) => this.#waiting.push({ resolve, reject }));
  }
}

// A delivery is the message plus the three ways to settle it. They are refused
// when acknowledgement is not manual: sending `basic.ack` for a tag the broker
// has already forgotten — which is what `noAck` means — would kill the channel.
function deliveryOf(channel: Channel, message: Message, manual: boolean): Delivery {
  const require = (): void => {
    if (!manual) {
      throw new RabbitError(
        "this subscription consumes without acknowledgements (noAck), so there is nothing to acknowledge",
        { code: RabbitErrorCode.Channel },
      );
    }
  };
  return {
    ...message,
    ack: (options) => {
      require();
      return channel.ack(message.deliveryTag, options);
    },
    nack: (options) => {
      require();
      return channel.nack(message.deliveryTag, options);
    },
    reject: (options) => {
      require();
      return channel.reject(message.deliveryTag, options);
    },
  };
}
