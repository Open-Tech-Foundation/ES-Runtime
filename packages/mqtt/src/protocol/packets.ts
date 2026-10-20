/**
 * The MQTT control packets: their fixed-header flags, their encoders, and the
 * decoder that turns bytes back into a packet.
 *
 * Every packet shares one shape — a first octet carrying the type in its high
 * nibble and four flags in its low nibble, then a variable byte integer of
 * Remaining Length, then the body. What varies is everything after that, and
 * the flags are only *sometimes* meaningful: PUBLISH reads all four (DUP, QoS,
 * RETAIN), PUBREL, SUBSCRIBE and UNSUBSCRIBE fix theirs at `0b0010`, and the
 * rest must send zero. The encoder sets the reserved bits and the decoder
 * refuses a packet where they are wrong, so a peer's mistake is a
 * `ProtocolError` and not a misread QoS.
 *
 * One codec serves both protocol levels. The difference is entirely in the
 * optional parts: 5.0 adds properties to most packets and reason codes to the
 * four acknowledgement packets, PUBREL, and DISCONNECT, while 3.1.1 has
 * neither and a larger set of bodies that are simply their packet identifier.
 * `decode` is told which level the connection negotiated, so the same bytes are
 * read the way the peer meant them.
 *
 * Remaining Length is not decoded here — the connection owns the framing, reads
 * the length itself, and hands this module exactly one packet's body. That is
 * what keeps a packet split across reads and a fixed header that arrives alone
 * the connection's problem, solved once, rather than each packet's.
 */

import { ProtocolError } from "./errors.js";
import { type Properties, readProperties, writeProperties } from "./properties.js";
import { validateTopicFilter, validateTopicName } from "./topic.js";
import { ByteReader, ByteWriter } from "./wire.js";

/** MQTT's protocol level: 4 is 3.1.1, 5 is 5.0. */
export type ProtocolLevel = 4 | 5;

/** The packet types, as the specification numbers them. */
export const PacketType = {
  Connect: 1,
  Connack: 2,
  Publish: 3,
  Puback: 4,
  Pubrec: 5,
  Pubrel: 6,
  Pubcomp: 7,
  Subscribe: 8,
  Suback: 9,
  Unsubscribe: 10,
  Unsuback: 11,
  Pingreq: 12,
  Pingresp: 13,
  Disconnect: 14,
  Auth: 15,
} as const;

export type PacketType = (typeof PacketType)[keyof typeof PacketType];

/** The last will and testament carried in a CONNECT. */
export interface Will {
  topic: string;
  payload: Uint8Array;
  qos: 0 | 1 | 2;
  retain: boolean;
  properties?: Properties;
}

export interface ConnectPacket {
  type: typeof PacketType.Connect;
  protocolName?: string;
  clientId: string;
  username?: string;
  password?: Uint8Array;
  keepAlive: number;
  cleanStart: boolean;
  will?: Will;
  properties?: Properties;
}

export interface ConnackPacket {
  type: typeof PacketType.Connack;
  sessionPresent: boolean;
  reasonCode: number;
  properties?: Properties;
}

export interface PublishPacket {
  type: typeof PacketType.Publish;
  topic: string;
  payload: Uint8Array;
  qos: 0 | 1 | 2;
  retain: boolean;
  dup: boolean;
  packetId?: number;
  properties?: Properties;
}

export interface AckPacket {
  type:
    | typeof PacketType.Puback
    | typeof PacketType.Pubrec
    | typeof PacketType.Pubrel
    | typeof PacketType.Pubcomp;
  packetId: number;
  reasonCode: number;
  properties?: Properties;
}

export interface SubscribeEntry {
  filter: string;
  qos: 0 | 1 | 2;
  noLocal?: boolean;
  retainAsPublished?: boolean;
  retainHandling?: 0 | 1 | 2;
}

export interface SubscribePacket {
  type: typeof PacketType.Subscribe;
  packetId: number;
  subscriptions: SubscribeEntry[];
  properties?: Properties;
}

export interface SubackPacket {
  type: typeof PacketType.Suback;
  packetId: number;
  reasons: number[];
  properties?: Properties;
}

export interface UnsubscribePacket {
  type: typeof PacketType.Unsubscribe;
  packetId: number;
  filters: string[];
  properties?: Properties;
}

export interface UnsubackPacket {
  type: typeof PacketType.Unsuback;
  packetId: number;
  reasons: number[];
  properties?: Properties;
}

export interface PingPacket {
  type: typeof PacketType.Pingreq | typeof PacketType.Pingresp;
}

export interface DisconnectPacket {
  type: typeof PacketType.Disconnect;
  reasonCode: number;
  properties?: Properties;
}

export interface AuthPacket {
  type: typeof PacketType.Auth;
  reasonCode: number;
  properties?: Properties;
}

/** Any packet this codec knows. */
export type Packet =
  | ConnectPacket
  | ConnackPacket
  | PublishPacket
  | AckPacket
  | SubscribePacket
  | SubackPacket
  | UnsubscribePacket
  | UnsubackPacket
  | PingPacket
  | DisconnectPacket
  | AuthPacket;

function body(build: (writer: ByteWriter) => number): { firstByte: number; body: Uint8Array } {
  const writer = new ByteWriter(64);
  const firstByte = build(writer);
  return { firstByte, body: writer.finish() };
}

/** Encodes one packet, fixed header and all, for the given protocol level. */
export function encodePacket(packet: Packet, level: ProtocolLevel = 5): Uint8Array {
  const { firstByte, body: bytes } = body((w) => encodeBody(w, packet, level));
  const out = new ByteWriter(bytes.length + 5);
  out.u8(firstByte).varint(bytes.length).raw(bytes);
  return out.finish();
}

function encodeBody(w: ByteWriter, packet: Packet, level: ProtocolLevel): number {
  switch (packet.type) {
    case PacketType.Connect: {
      w.utf8(packet.protocolName ?? "MQTT").u8(level);
      let flags = 0;
      if (packet.cleanStart) flags |= 0x02;
      if (packet.will !== undefined) {
        flags |= 0x04 | ((packet.will.qos & 3) << 3);
        if (packet.will.retain) flags |= 0x20;
      }
      if (packet.password !== undefined) flags |= 0x40;
      if (packet.username !== undefined) flags |= 0x80;
      w.u8(flags).u16(packet.keepAlive);
      if (level === 5) writeProperties(w, packet.properties);
      w.utf8(packet.clientId);
      if (packet.will !== undefined) {
        if (level === 5) writeProperties(w, packet.will.properties);
        w.utf8(packet.will.topic).binary(packet.will.payload);
      }
      if (packet.username !== undefined) w.utf8(packet.username);
      if (packet.password !== undefined) w.binary(packet.password);
      return PacketType.Connect << 4;
    }
    case PacketType.Connack: {
      w.u8(packet.sessionPresent ? 1 : 0).u8(packet.reasonCode);
      if (level === 5) writeProperties(w, packet.properties);
      return PacketType.Connack << 4;
    }
    case PacketType.Publish: {
      // An empty topic is only legal on 5.0 with a Topic Alias standing in.
      const aliased =
        level === 5 && packet.topic === "" && packet.properties?.topicAlias !== undefined;
      if (!aliased) validateTopicName(packet.topic);
      if (packet.qos === 0 && packet.dup) {
        throw new ProtocolError("DUP must be 0 on a QoS 0 PUBLISH");
      }
      w.utf8(packet.topic);
      if (packet.qos > 0) {
        if (packet.packetId === undefined || packet.packetId === 0) {
          throw new ProtocolError("a QoS 1 or 2 PUBLISH needs a packet identifier");
        }
        w.u16(packet.packetId);
      }
      if (level === 5) writeProperties(w, packet.properties);
      w.raw(packet.payload);
      const flags = (packet.dup ? 0x08 : 0) | (packet.qos << 1) | (packet.retain ? 0x01 : 0);
      return (PacketType.Publish << 4) | flags;
    }
    case PacketType.Puback:
    case PacketType.Pubrec:
    case PacketType.Pubrel:
    case PacketType.Pubcomp: {
      w.u16(packet.packetId);
      if (level === 5) {
        w.u8(packet.reasonCode);
        writeProperties(w, packet.properties);
      }
      const flags = packet.type === PacketType.Pubrel ? 0x02 : 0;
      return (packet.type << 4) | flags;
    }
    case PacketType.Subscribe: {
      if (packet.subscriptions.length === 0) {
        throw new ProtocolError("a SUBSCRIBE needs at least one topic filter");
      }
      w.u16(packet.packetId);
      if (level === 5) writeProperties(w, packet.properties);
      for (const sub of packet.subscriptions) {
        validateTopicFilter(sub.filter);
        w.utf8(sub.filter);
        let opts = sub.qos & 3;
        if (sub.noLocal === true) opts |= 0x04;
        if (sub.retainAsPublished === true) opts |= 0x08;
        opts |= ((sub.retainHandling ?? 0) & 3) << 4;
        w.u8(opts);
      }
      return (PacketType.Subscribe << 4) | 0x02;
    }
    case PacketType.Suback: {
      w.u16(packet.packetId);
      if (level === 5) writeProperties(w, packet.properties);
      for (const reason of packet.reasons) w.u8(reason);
      return PacketType.Suback << 4;
    }
    case PacketType.Unsubscribe: {
      if (packet.filters.length === 0) {
        throw new ProtocolError("an UNSUBSCRIBE needs at least one topic filter");
      }
      w.u16(packet.packetId);
      if (level === 5) writeProperties(w, packet.properties);
      for (const filter of packet.filters) {
        validateTopicFilter(filter);
        w.utf8(filter);
      }
      return (PacketType.Unsubscribe << 4) | 0x02;
    }
    case PacketType.Unsuback: {
      w.u16(packet.packetId);
      if (level === 5) {
        writeProperties(w, packet.properties);
        for (const reason of packet.reasons) w.u8(reason);
      }
      return PacketType.Unsuback << 4;
    }
    case PacketType.Pingreq:
    case PacketType.Pingresp:
      return packet.type << 4;
    case PacketType.Disconnect: {
      if (level === 5) {
        w.u8(packet.reasonCode);
        writeProperties(w, packet.properties);
      }
      return PacketType.Disconnect << 4;
    }
    case PacketType.Auth: {
      w.u8(packet.reasonCode);
      writeProperties(w, packet.properties);
      return PacketType.Auth << 4;
    }
  }
}

/**
 * Decodes one packet's body, given its first octet and the negotiated level.
 *
 * The connection reads the fixed header and Remaining Length and passes the
 * body here; this is where a body shorter or longer than its packet type allows
 * becomes a `ProtocolError`.
 */
export function decodePacketBody(
  firstByte: number,
  bytes: Uint8Array,
  level: ProtocolLevel = 5,
): Packet {
  const type = (firstByte >> 4) as PacketType;
  const flags = firstByte & 0x0f;
  const reader = new ByteReader(bytes);
  const packet = decodeBody(reader, type, flags, level);
  if (!reader.done) {
    throw new ProtocolError(`a ${packetName(type)} packet had ${reader.remaining} bytes too many`);
  }
  return packet;
}

/** Decodes a whole packet, fixed header and all (used by tests and a stub broker). */
export function decodePacket(bytes: Uint8Array, level: ProtocolLevel = 5): Packet {
  const reader = new ByteReader(bytes);
  const firstByte = reader.u8();
  const length = reader.varint();
  const bodyBytes = reader.bytes(length);
  if (!reader.done) throw new ProtocolError("trailing bytes after a packet");
  return decodePacketBody(firstByte, bodyBytes, level);
}

function props(reader: ByteReader, level: ProtocolLevel): Properties | undefined {
  return level === 5 ? readProperties(reader) : undefined;
}

function withProps<T extends object>(base: T, properties: Properties | undefined): T {
  return properties === undefined ? base : { ...base, properties };
}

function decodeBody(r: ByteReader, type: PacketType, flags: number, level: ProtocolLevel): Packet {
  switch (type) {
    case PacketType.Connect: {
      const protocolName = r.utf8();
      if (protocolName !== "MQTT" && protocolName !== "MQIsdp") {
        throw new ProtocolError(`unknown protocol name ${JSON.stringify(protocolName)}`);
      }
      const protocolLevel = r.u8();
      if (protocolLevel !== 4 && protocolLevel !== 5) {
        throw new ProtocolError(`unsupported protocol level ${protocolLevel}`);
      }
      const cf = r.u8();
      if ((cf & 0x01) !== 0) throw new ProtocolError("the CONNECT reserved flag must be 0");
      const keepAlive = r.u16();
      const cleanStart = (cf & 0x02) !== 0;
      const willFlag = (cf & 0x04) !== 0;
      const willQos = ((cf >> 3) & 3) as 0 | 1 | 2;
      const willRetain = (cf & 0x20) !== 0;
      const hasPassword = (cf & 0x40) !== 0;
      const hasUsername = (cf & 0x80) !== 0;
      if (!willFlag && (willQos !== 0 || willRetain)) {
        throw new ProtocolError("will QoS and retain must be 0 when there is no will");
      }
      if (willFlag && (cf & 0x04) === 0) throw new ProtocolError("malformed CONNECT will flags");
      const properties = props(r, protocolLevel);
      const clientId = r.utf8();
      let will: Will | undefined;
      if (willFlag) {
        const willProperties = props(r, protocolLevel);
        const topic = r.utf8();
        validateTopicName(topic);
        const payload = r.binary();
        will =
          willProperties === undefined
            ? { topic, payload, qos: willQos, retain: willRetain }
            : { topic, payload, qos: willQos, retain: willRetain, properties: willProperties };
      }
      const username = hasUsername ? r.utf8() : undefined;
      const password = hasPassword ? r.binary() : undefined;
      return {
        type: PacketType.Connect,
        protocolName,
        clientId,
        ...(username === undefined ? {} : { username }),
        ...(password === undefined ? {} : { password }),
        keepAlive,
        cleanStart,
        ...(will === undefined ? {} : { will }),
        ...(properties === undefined ? {} : { properties }),
      };
    }
    case PacketType.Connack: {
      const sessionPresentByte = r.u8();
      const sessionPresent = sessionPresentByte !== 0;
      const reasonCode = r.u8();
      return withProps(
        { type: PacketType.Connack, sessionPresent, reasonCode } as ConnackPacket,
        props(r, level),
      );
    }
    case PacketType.Publish: {
      const dup = (flags & 0x08) !== 0;
      const qos = ((flags >> 1) & 3) as 0 | 1 | 2 | 3;
      const retain = (flags & 0x01) !== 0;
      if (qos === 3) throw new ProtocolError("a PUBLISH QoS of 3 is malformed");
      const topic = r.utf8();
      // On 5.0 an empty topic means "use the Topic Alias property", which the
      // connection resolves; anywhere else it is malformed.
      if (!(level === 5 && topic === "")) validateTopicName(topic);
      let packetId: number | undefined;
      if (qos > 0) {
        packetId = r.u16();
        if (packetId === 0)
          throw new ProtocolError("a QoS 1 or 2 PUBLISH needs a packet identifier");
      } else if (dup) {
        throw new ProtocolError("DUP must be 0 on a QoS 0 PUBLISH");
      }
      const properties = props(r, level);
      const payload = r.rest();
      return {
        type: PacketType.Publish,
        topic,
        payload,
        qos,
        retain,
        dup,
        ...(packetId === undefined ? {} : { packetId }),
        ...(properties === undefined ? {} : { properties }),
      };
    }
    case PacketType.Puback:
    case PacketType.Pubrec:
    case PacketType.Pubrel:
    case PacketType.Pubcomp: {
      const expected = type === PacketType.Pubrel ? 0x02 : 0;
      if (flags !== expected) {
        throw new ProtocolError(`a ${packetName(type)} packet had the wrong fixed-header flags`);
      }
      const packetId = r.u16();
      let reasonCode = 0;
      let properties: Properties | undefined;
      if (level === 5 && r.remaining > 0) reasonCode = r.u8();
      if (level === 5 && r.remaining > 0) properties = readProperties(r);
      return withProps({ type, packetId, reasonCode } as AckPacket, properties);
    }
    case PacketType.Subscribe: {
      if (flags !== 0x02)
        throw new ProtocolError("a SUBSCRIBE packet had the wrong fixed-header flags");
      const packetId = r.u16();
      const properties = props(r, level);
      const subscriptions: SubscribeEntry[] = [];
      while (!r.done) {
        const filter = r.utf8();
        validateTopicFilter(filter);
        const opts = r.u8();
        const qos = (opts & 3) as 0 | 1 | 2 | 3;
        if (qos === 3) throw new ProtocolError("a subscription QoS of 3 is malformed");
        const entry: SubscribeEntry = { filter, qos };
        if ((opts & 0x04) !== 0) entry.noLocal = true;
        if ((opts & 0x08) !== 0) entry.retainAsPublished = true;
        const retainHandling = ((opts >> 4) & 3) as 0 | 1 | 2 | 3;
        if (retainHandling === 3) throw new ProtocolError("a retain-handling of 3 is malformed");
        entry.retainHandling = retainHandling;
        subscriptions.push(entry);
      }
      if (subscriptions.length === 0)
        throw new ProtocolError("a SUBSCRIBE needs at least one filter");
      return withProps(
        { type: PacketType.Subscribe, packetId, subscriptions } as SubscribePacket,
        properties,
      );
    }
    case PacketType.Suback: {
      const packetId = r.u16();
      const properties = props(r, level);
      const reasons: number[] = [];
      while (!r.done) reasons.push(r.u8());
      return withProps({ type: PacketType.Suback, packetId, reasons } as SubackPacket, properties);
    }
    case PacketType.Unsubscribe: {
      if (flags !== 0x02)
        throw new ProtocolError("an UNSUBSCRIBE packet had the wrong fixed-header flags");
      const packetId = r.u16();
      const properties = props(r, level);
      const filters: string[] = [];
      while (!r.done) {
        const filter = r.utf8();
        validateTopicFilter(filter);
        filters.push(filter);
      }
      if (filters.length === 0) throw new ProtocolError("an UNSUBSCRIBE needs at least one filter");
      return withProps(
        { type: PacketType.Unsubscribe, packetId, filters } as UnsubscribePacket,
        properties,
      );
    }
    case PacketType.Unsuback: {
      const packetId = r.u16();
      const properties = props(r, level);
      const reasons: number[] = [];
      if (level === 5) while (!r.done) reasons.push(r.u8());
      return withProps(
        { type: PacketType.Unsuback, packetId, reasons } as UnsubackPacket,
        properties,
      );
    }
    case PacketType.Pingreq:
    case PacketType.Pingresp:
      if (flags !== 0)
        throw new ProtocolError(`a ${packetName(type)} packet had the wrong fixed-header flags`);
      return { type };
    case PacketType.Disconnect: {
      let reasonCode = 0;
      let properties: Properties | undefined;
      if (level === 5 && r.remaining > 0) reasonCode = r.u8();
      if (level === 5 && r.remaining > 0) properties = readProperties(r);
      return withProps({ type: PacketType.Disconnect, reasonCode } as DisconnectPacket, properties);
    }
    case PacketType.Auth: {
      const reasonCode = r.remaining > 0 ? r.u8() : 0;
      const properties = r.remaining > 0 ? readProperties(r) : undefined;
      return withProps({ type: PacketType.Auth, reasonCode } as AuthPacket, properties);
    }
    default:
      throw new ProtocolError(`unknown MQTT packet type ${type}`);
  }
}

const NAMES: Record<number, string> = {
  1: "CONNECT",
  2: "CONNACK",
  3: "PUBLISH",
  4: "PUBACK",
  5: "PUBREC",
  6: "PUBREL",
  7: "PUBCOMP",
  8: "SUBSCRIBE",
  9: "SUBACK",
  10: "UNSUBSCRIBE",
  11: "UNSUBACK",
  12: "PINGREQ",
  13: "PINGRESP",
  14: "DISCONNECT",
  15: "AUTH",
};

/** The name of a packet type, for an error message. */
export function packetName(type: number): string {
  return NAMES[type] ?? `packet ${type}`;
}
