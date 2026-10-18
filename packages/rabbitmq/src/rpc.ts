/**
 * Request/reply, over RabbitMQ's `direct reply-to`.
 *
 * A client that calls a service needs an answer routed back to it and to no one
 * else. The general AMQP way is to declare a queue per caller, bind it, and
 * sweep up after; RabbitMQ's `direct reply-to` does away with all of that. The
 * client consumes the pseudo-queue `amq.rabbitmq.reply-to` (which is never
 * declared and only accepts `noAck`), publishes its request with that as
 * `replyTo` and a `correlationId`, and the broker rewrites `replyTo` on the way
 * to the server so the server's answer comes straight back to this channel. No
 * reply queue exists to name, to expire, or to leak.
 *
 * ```js
 * const channel = await connection.channel();
 * const rpc = await RpcClient.open(channel);
 * const reply = await rpc.call("", "fib", "10");
 * console.log(new TextDecoder().decode(reply.body));
 * ```
 *
 * Three things the layer decides, all of them consequences of there being one
 * reply pseudo-queue per channel:
 *
 * - **One client per channel.** `amq.rabbitmq.reply-to` allows a single
 *   consumer, and the answer comes back on the channel that consumed it, so a
 *   second {@link RpcClient} on the same channel is not a second conversation.
 * - **Replies are matched by correlation id.** Many calls are in flight at once
 *   because they are told apart by the id, not by ordering; a reply whose id is
 *   not pending — a duplicate, or one that arrived after its call timed out —
 *   is dropped.
 * - **A call is only as reliable as its timeout.** The pseudo-queue is not
 *   durable and has no dead-lettering, so a server that never answers leaves a
 *   call waiting until its timeout; there is no queue to inspect afterwards.
 *   `timeout: 0` waits forever, deliberately.
 */

import type { Channel, Message } from "./channel.js";
import { type Subscription, subscribe } from "./consumer.js";
import { RabbitError, RabbitErrorCode } from "./errors.js";
import type { BasicProperties } from "./protocol/properties.js";

/**
 * RabbitMQ's direct reply-to pseudo-queue. Consuming it is what makes a channel
 * a reply address; it cannot be declared, bound, or published to directly.
 */
export const DIRECT_REPLY_TO = "amq.rabbitmq.reply-to";

/** How long a call waits for a reply, in ms, unless told otherwise. */
const DEFAULT_TIMEOUT = 30_000;

export interface RpcOptions {
  /**
   * How long each call waits for an answer before rejecting, in ms. `0` waits
   * forever. Defaults to 30000.
   */
  timeout?: number;
  /** The reply consumer's tag. Unique per channel; generated otherwise. */
  consumerTag?: string;
}

/**
 * What {@link RpcClient.call} takes: the request's properties, and where and how
 * to send it. `replyTo` is set by the client and ignored here; `correlationId`
 * is generated when not given.
 */
export interface RpcCallOptions extends BasicProperties {
  /** The exchange to publish to. The default exchange routes by queue name. */
  exchange?: string;
  /** How long this call waits, in ms, overriding the client's. `0` waits forever. */
  timeout?: number;
  /**
   * Ask the broker to return an unroutable request rather than drop it. A
   * returned request is never answered, so the call then waits out its
   * timeout; the return itself reaches the channel's `onReturn`.
   */
  mandatory?: boolean;
}

interface Pending {
  resolve: (message: Message) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout> | null;
}

let nextCorrelation = 1;

/**
 * A request/reply client over one channel.
 *
 * Open one per channel; it consumes the channel's reply pseudo-queue and routes
 * answers back to the calls that are waiting. Closing it cancels that consumer
 * and fails every call still in flight.
 */
export class RpcClient {
  readonly #channel: Channel;
  readonly #timeout: number;
  readonly #pending = new Map<string, Pending>();
  #subscription: Subscription | null = null;
  #closed = false;

  private constructor(channel: Channel, options: RpcOptions) {
    this.#channel = channel;
    this.#timeout = options.timeout ?? DEFAULT_TIMEOUT;
  }

  /**
   * Consumes the channel's reply pseudo-queue and answers with a client ready
   * to {@link call}. The consumer is started before it returns, so a reply
   * cannot be missed for a call made afterwards.
   */
  static async open(channel: Channel, options: RpcOptions = {}): Promise<RpcClient> {
    const client = new RpcClient(channel, options);
    client.#subscription = await subscribe(channel, DIRECT_REPLY_TO, {
      noAck: true,
      ...(options.consumerTag === undefined ? {} : { consumerTag: options.consumerTag }),
    });
    void client.#pump();
    return client;
  }

  /** Whether it has been closed, or its channel has ended. */
  get closed(): boolean {
    return this.#closed;
  }

  /** How many calls are waiting for an answer. */
  get pending(): number {
    return this.#pending.size;
  }

  /**
   * Publishes a request and answers with the reply.
   *
   * The request carries `replyTo: amq.rabbitmq.reply-to` and a correlation id
   * (generated, or the one given), and the answer is the message the server
   * publishes back to that id. Awaiting the call awaits the reply.
   */
  call(
    routingKey: string,
    body: string | Uint8Array,
    options: RpcCallOptions = {},
  ): Promise<Message> {
    if (this.#closed) {
      return Promise.reject(
        new RabbitError("the rpc client is closed", { code: RabbitErrorCode.Closed }),
      );
    }
    const {
      exchange = "",
      timeout = this.#timeout,
      mandatory = false,
      correlationId = `esrun-rpc-${nextCorrelation++}`,
      ...properties
    } = options;
    if (this.#pending.has(correlationId)) {
      return Promise.reject(
        new RabbitError(`a call with correlation id ${correlationId} is already in flight`, {
          code: RabbitErrorCode.Channel,
        }),
      );
    }

    // The waiter is registered before the request goes out, so a reply that
    // races the publish still finds its call.
    const reply = this.#await(correlationId, timeout);
    this.#channel
      .publish(exchange, routingKey, body, {
        ...properties,
        correlationId,
        replyTo: DIRECT_REPLY_TO,
        mandatory,
      })
      .catch((e: unknown) => this.#reject(correlationId, this.#wrap(e)));
    return reply;
  }

  /**
   * Cancels the reply consumer and fails every call still waiting.
   *
   * Answers once the broker has stopped the consumer. A call whose reply had
   * already arrived is unaffected.
   */
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    this.#settleAll(
      new RabbitError("the rpc client was closed before the reply arrived", {
        code: RabbitErrorCode.Closed,
      }),
    );
    try {
      await this.#subscription?.cancel();
    } catch {
      // The channel is gone, which is the end the caller asked for anyway.
    }
  }

  // -- replies --------------------------------------------------------------

  // Reads the reply consumer for as long as it runs. A reply is routed to the
  // call whose correlation id it names; anything else — an unknown id, a
  // delivery with no id at all — is dropped. The iteration ends normally when
  // the consumer is cancelled — by the broker, or by {@link close} — and throws
  // when the channel itself died, carrying its reason.
  async #pump(): Promise<void> {
    try {
      for await (const delivery of this.#subscription as Subscription) {
        const id = delivery.properties.correlationId;
        if (id === undefined) continue;
        const pending = this.#pending.get(id);
        if (pending === undefined) continue;
        this.#pending.delete(id);
        if (pending.timer !== null) clearTimeout(pending.timer);
        pending.resolve(delivery);
      }
      // A broker can cancel a consumer without closing its channel. The reply
      // path is gone in that case too, so fail calls now instead of leaving
      // them to time out and refusing no future calls.
      if (!this.#closed) {
        this.#closed = true;
        this.#settleAll(
          new RabbitError("the rpc reply consumer ended before the reply arrived", {
            code: RabbitErrorCode.Closed,
          }),
        );
      }
    } catch (e) {
      this.#closed = true;
      this.#settleAll(e instanceof Error ? e : this.#wrap(e));
    }
  }

  #await(correlationId: string, timeout: number): Promise<Message> {
    return new Promise<Message>((resolve, reject) => {
      const timer =
        timeout > 0
          ? setTimeout(() => {
              if (this.#pending.delete(correlationId)) {
                reject(
                  new RabbitError(`no reply to ${correlationId} within ${timeout}ms`, {
                    code: RabbitErrorCode.Timeout,
                  }),
                );
              }
            }, timeout)
          : null;
      this.#pending.set(correlationId, { resolve, reject, timer });
    });
  }

  #reject(correlationId: string, error: Error): void {
    const pending = this.#pending.get(correlationId);
    if (pending === undefined) return;
    this.#pending.delete(correlationId);
    if (pending.timer !== null) clearTimeout(pending.timer);
    pending.reject(error);
  }

  #settleAll(error: Error): void {
    for (const [id, pending] of [...this.#pending]) {
      this.#pending.delete(id);
      if (pending.timer !== null) clearTimeout(pending.timer);
      pending.reject(error);
    }
  }

  #wrap(e: unknown): RabbitError {
    if (e instanceof RabbitError) return e;
    return new RabbitError(e instanceof Error ? e.message : String(e), {
      code: RabbitErrorCode.Channel,
      cause: e,
    });
  }
}
