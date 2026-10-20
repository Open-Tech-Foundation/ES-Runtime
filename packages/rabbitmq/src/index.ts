/**
 * `@opentf/esrun-rabbitmq` — a RabbitMQ client speaking AMQP 0-9-1, in
 * JavaScript over `runtime:net` (DECISIONS D171).
 *
 * The wire protocol lives in `./protocol`, the connection handshake and
 * heartbeats in `./connection`, channels and topology in `./channel`, consuming
 * in `./consumer`, request/reply over `direct reply-to` in `./rpc`, and the
 * connection/channel pool in `./pool`. This module is the whole public surface;
 * nothing is registered by importing it, and no capability is added — the client
 * reaches the broker through `runtime:net`'s `connect`, which the specifier
 * already needs.
 *
 * ```js
 * import { openConnection, subscribe } from "@opentf/esrun-rabbitmq";
 *
 * const connection = await openConnection("amqp://guest:guest@localhost/");
 * const channel = await connection.channel();
 * await channel.queueDeclare("jobs", { durable: true });
 * await channel.publish("", "jobs", "hello");
 *
 * const subscription = await subscribe(channel, "jobs");
 * for await (const delivery of subscription) {
 *   console.log(new TextDecoder().decode(delivery.body));
 *   await delivery.ack();
 * }
 *
 * await connection.close();
 * ```
 *
 * A {@link Pool} holds several connections and dials a fresh one when one dies,
 * which is the whole of its recovery — re-declaring topology after a lost
 * connection is the application's, from {@link PoolOptions.onConnect}:
 *
 * ```js
 * import { Pool } from "@opentf/esrun-rabbitmq";
 *
 * const pool = new Pool("amqp://localhost/", { size: 4 });
 * await pool.withChannel((channel) => channel.publish("", "jobs", "hello"));
 * await pool.close();
 * ```
 *
 * Request/reply over the broker's `direct reply-to` is one call:
 *
 * ```js
 * import { openConnection, RpcClient } from "@opentf/esrun-rabbitmq";
 *
 * const rpc = await RpcClient.open(await (await openConnection("amqp://localhost/")).channel());
 * const answer = await rpc.call("service", "ping");
 * ```
 */

export {
  type AckOptions,
  type BindOptions,
  Channel,
  type Delivery,
  type ExchangeDeleteOptions,
  type ExchangeOptions,
  type GetOptions,
  type Message,
  type NackOptions,
  type PublishOptions,
  type QosOptions,
  type QueueDeclaration,
  type QueueDeleteOptions,
  type QueueOptions,
  type RejectOptions,
} from "./channel.js";
export {
  type ChannelHandler,
  Connection,
  type ConnectionSettings,
  DEFAULT_HEARTBEAT,
  type ServerInfo,
} from "./connection.js";
export {
  type Cancellation,
  type ConsumeOptions,
  DEFAULT_PREFETCH,
  Subscription,
  subscribe,
} from "./consumer.js";
export { ProtocolError, RabbitError, RabbitErrorCode, type RabbitErrorOptions } from "./errors.js";
export {
  type BrokerAddress,
  type OpenSettings,
  openConnection,
  Pool,
  type PoolOptions,
  parseAmqpUrl,
} from "./pool.js";
export type { FieldTable, FieldValue } from "./protocol/fields.js";
export type { DecodedMethod } from "./protocol/methods.js";
export type { BasicProperties, ContentHeader } from "./protocol/properties.js";
export { DIRECT_REPLY_TO, type RpcCallOptions, RpcClient, type RpcOptions } from "./rpc.js";
export {
  openSession,
  type ReconnectOptions,
  RecoveringChannel,
  type RecoveringChannelOptions,
  RecoveringSubscription,
  Session,
  type SessionOptions,
} from "./session.js";
export { type NetTransportOptions, netTransport, type Transport } from "./transport.js";
