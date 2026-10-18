/**
 * `@opentf/esrun-mqtt` — an MQTT client speaking MQTT 5.0, with a fallback to
 * 3.1.1, in JavaScript over `runtime:net` (DECISIONS D172).
 *
 * The wire protocol lives in `./protocol` — the variable byte integer, the
 * string and binary forms, the 5.0 property set, topic validation, and the
 * packet codec. The `Transport` seam lives in `./transport`, and the connection
 * and client layers build on both. This module is the whole public surface;
 * nothing is registered by importing it, and no capability is added — the client
 * reaches the broker through `runtime:net`'s `connect`, which the specifier
 * already needs.
 *
 * The protocol layer is exported so a caller can reach the codec — a test
 * harness, a stub broker — without the package re-exporting anything it does
 * not name here.
 */

export {
  Message,
  MqttClient,
  type MqttClientOptions,
  type PublishOptions,
  type SubscribeOptions,
  Subscription,
} from "./client.js";
export {
  Connection,
  type ConnectionHandlers,
  type ConnectionSettings,
  DEFAULT_KEEP_ALIVE,
  DEFAULT_MQTT_PORT,
  DEFAULT_MQTTS_PORT,
  type MqttAddress,
  openConnection,
  parseMqttUrl,
} from "./connection.js";
export { MqttError, MqttErrorCode, type MqttErrorOptions, ProtocolError } from "./errors.js";
export {
  decodePacket,
  decodePacketBody,
  encodePacket,
  type Packet,
  PacketType,
  type ProtocolLevel,
  type Will,
} from "./protocol/packets.js";
export type { Properties } from "./protocol/properties.js";
export { isErrorReason, reasonName } from "./protocol/reason.js";
export { topicMatches, validateTopicFilter, validateTopicName } from "./protocol/topic.js";
export { type NetTransportOptions, netTransport, type Transport } from "./transport.js";
