/**
 * `@opentf/esrun-nats` — a NATS client speaking core NATS over `runtime:net`
 * (DECISIONS D173).
 *
 * The wire protocol lives in `./protocol` — the line and byte-counted command
 * codec, the streaming parser, subject rules and the `NATS/1.0` header block.
 * The `Transport` seam lives in `./transport`, and the connection and client
 * layers build on both. This module is the whole public surface; nothing is
 * registered by importing it, and no capability is added — the client reaches
 * the server through `runtime:net`'s `connect`, which the specifier already
 * needs.
 *
 * ```js
 * import { NatsClient } from "@opentf/esrun-nats";
 *
 * const client = await NatsClient.connect("nats://localhost:4222");
 * const subscription = client.subscribe("events.>");
 * await client.publish("events.user.created", JSON.stringify({ id: 1 }));
 *
 * for await (const message of subscription) {
 *   console.log(message.subject, message.json());
 * }
 *
 * await client.close();
 * ```
 *
 * The protocol layer is exported so a caller can reach the codec — a test
 * harness, a stub server — without the package re-exporting anything it does
 * not name here.
 */

export {
  DEFAULT_PENDING_BYTES,
  DEFAULT_PENDING_MESSAGES,
  DEFAULT_REQUEST_TIMEOUT,
  NatsClient,
  type NatsClientOptions,
  NatsMessage,
  type PublishOptions,
  type RequestOptions,
  type SubscribeOptions,
  Subscription,
} from "./client.js";
export {
  Connection,
  type ConnectionHandlers,
  type ConnectionSettings,
  DEFAULT_MAX_RECONNECT_ATTEMPTS,
  DEFAULT_NATS_PORT,
  DEFAULT_PING_INTERVAL,
  DEFAULT_RECONNECT_JITTER,
  DEFAULT_RECONNECT_JITTER_TLS,
  DEFAULT_RECONNECT_WAIT,
  DEFAULT_TIMEOUT,
  type Dialer,
  type IncomingMessage,
  type NatsAddress,
  openConnection,
  openWithDial,
  parseNatsServer,
  parseNatsUrl,
  type ReconnectSettings,
} from "./connection.js";
export { NatsError, NatsErrorCode, type NatsErrorOptions, ProtocolError } from "./errors.js";
export {
  type DecodedHeaders,
  decodeHeaders,
  encodeHeaders,
  type HeaderInit,
  type HeaderStatus,
} from "./protocol/headers.js";
export { createInbox, subjectMatches, validateQueue, validateSubject } from "./protocol/subject.js";
export {
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
  parseInfo,
  type ServerCommand,
  type ServerInfo,
} from "./protocol/wire.js";
export {
  NetTransport,
  type NetTransportOptions,
  netTransport,
  type Transport,
} from "./transport.js";
