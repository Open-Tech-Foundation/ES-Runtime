/**
 * The MQTT 5.0 property set, and the one place its wire form is decoded.
 *
 * MQTT 5.0 hangs an optional bag of typed properties off most packets. The bag
 * is a variable byte integer of length followed by that many bytes, and inside
 * it each property is its own identifier (another variable byte integer) and a
 * value whose *form* follows from the identifier — a `byte`, a two- or
 * four-octet integer, a variable byte integer, a UTF-8 string, binary data, or
 * the UTF-8 pair of a user property. There is no type octet: the identifier is
 * the only thing that says how many octets come next, which is exactly why a
 * decoder that guessed would misread a packet. The type is looked up here.
 *
 * Two properties are repeatable — user properties and subscription identifiers
 * may appear more than once — and every other one may appear at most once per
 * packet (MQTT-2.2.2.2). Both rules are enforced on read, so a packet that
 * breaks either is a `ProtocolError` rather than a value silently overwritten.
 *
 * This is the whole property set, not only the subset this client fills in.
 * A broker may send any of them, and a decoder with holes would fail on a legal
 * packet it simply did not model; the caller decides which it cares about.
 */

import { ProtocolError } from "./errors.js";
import { type ByteReader, ByteWriter } from "./wire.js";

/** Property identifiers, as the specification numbers them. */
export const PropertyId = {
  PayloadFormatIndicator: 0x01,
  MessageExpiryInterval: 0x02,
  ContentType: 0x03,
  ResponseTopic: 0x08,
  CorrelationData: 0x09,
  SubscriptionIdentifier: 0x0b,
  SessionExpiryInterval: 0x11,
  AssignedClientIdentifier: 0x12,
  ServerKeepAlive: 0x13,
  AuthenticationMethod: 0x15,
  AuthenticationData: 0x16,
  RequestProblemInformation: 0x17,
  WillDelayInterval: 0x18,
  RequestResponseInformation: 0x19,
  ResponseInformation: 0x1a,
  ServerReference: 0x1c,
  ReasonString: 0x1f,
  ReceiveMaximum: 0x21,
  TopicAliasMaximum: 0x22,
  TopicAlias: 0x23,
  MaximumQoS: 0x24,
  RetainAvailable: 0x25,
  UserProperty: 0x26,
  MaximumPacketSize: 0x27,
  WildcardSubscriptionAvailable: 0x28,
  SubscriptionIdentifierAvailable: 0x29,
  SharedSubscriptionAvailable: 0x2a,
} as const;

/** A property bag, in the terms the specification names its fields. */
export interface Properties {
  /** 0 = bytes, 1 = UTF-8 text (PUBLISH). */
  payloadFormatIndicator?: number;
  /** Seconds, 4 octets (PUBLISH, will). */
  messageExpiryInterval?: number;
  contentType?: string;
  responseTopic?: string;
  correlationData?: Uint8Array;
  /** Repeatable; each is a variable byte integer. */
  subscriptionIdentifiers?: number[];
  /** Seconds, 4 octets (CONNECT, CONNACK, DISCONNECT). */
  sessionExpiryInterval?: number;
  assignedClientIdentifier?: string;
  serverKeepAlive?: number;
  authenticationMethod?: string;
  authenticationData?: Uint8Array;
  requestProblemInformation?: number;
  /** Seconds, 4 octets (will). */
  willDelayInterval?: number;
  requestResponseInformation?: number;
  responseInformation?: string;
  serverReference?: string;
  reasonString?: string;
  receiveMaximum?: number;
  topicAliasMaximum?: number;
  topicAlias?: number;
  maximumQoS?: number;
  retainAvailable?: number;
  /** Repeatable; a UTF-8 pair per entry. */
  userProperties?: Array<[string, string]>;
  maximumPacketSize?: number;
  wildcardSubscriptionAvailable?: number;
  subscriptionIdentifierAvailable?: number;
  sharedSubscriptionAvailable?: number;
}

type PropType = "byte" | "u16" | "u32" | "varint" | "utf8" | "binary" | "pair";

interface PropDef {
  id: number;
  name: keyof Properties;
  type: PropType;
  repeat?: boolean;
}

const DEFS: readonly PropDef[] = [
  { id: PropertyId.PayloadFormatIndicator, name: "payloadFormatIndicator", type: "byte" },
  { id: PropertyId.MessageExpiryInterval, name: "messageExpiryInterval", type: "u32" },
  { id: PropertyId.ContentType, name: "contentType", type: "utf8" },
  { id: PropertyId.ResponseTopic, name: "responseTopic", type: "utf8" },
  { id: PropertyId.CorrelationData, name: "correlationData", type: "binary" },
  {
    id: PropertyId.SubscriptionIdentifier,
    name: "subscriptionIdentifiers",
    type: "varint",
    repeat: true,
  },
  { id: PropertyId.SessionExpiryInterval, name: "sessionExpiryInterval", type: "u32" },
  { id: PropertyId.AssignedClientIdentifier, name: "assignedClientIdentifier", type: "utf8" },
  { id: PropertyId.ServerKeepAlive, name: "serverKeepAlive", type: "u16" },
  { id: PropertyId.AuthenticationMethod, name: "authenticationMethod", type: "utf8" },
  { id: PropertyId.AuthenticationData, name: "authenticationData", type: "binary" },
  { id: PropertyId.RequestProblemInformation, name: "requestProblemInformation", type: "byte" },
  { id: PropertyId.WillDelayInterval, name: "willDelayInterval", type: "u32" },
  { id: PropertyId.RequestResponseInformation, name: "requestResponseInformation", type: "byte" },
  { id: PropertyId.ResponseInformation, name: "responseInformation", type: "utf8" },
  { id: PropertyId.ServerReference, name: "serverReference", type: "utf8" },
  { id: PropertyId.ReasonString, name: "reasonString", type: "utf8" },
  { id: PropertyId.ReceiveMaximum, name: "receiveMaximum", type: "u16" },
  { id: PropertyId.TopicAliasMaximum, name: "topicAliasMaximum", type: "u16" },
  { id: PropertyId.TopicAlias, name: "topicAlias", type: "u16" },
  { id: PropertyId.MaximumQoS, name: "maximumQoS", type: "byte" },
  { id: PropertyId.RetainAvailable, name: "retainAvailable", type: "byte" },
  { id: PropertyId.UserProperty, name: "userProperties", type: "pair", repeat: true },
  { id: PropertyId.MaximumPacketSize, name: "maximumPacketSize", type: "u32" },
  {
    id: PropertyId.WildcardSubscriptionAvailable,
    name: "wildcardSubscriptionAvailable",
    type: "byte",
  },
  {
    id: PropertyId.SubscriptionIdentifierAvailable,
    name: "subscriptionIdentifierAvailable",
    type: "byte",
  },
  { id: PropertyId.SharedSubscriptionAvailable, name: "sharedSubscriptionAvailable", type: "byte" },
];

const BY_ID: ReadonlyMap<number, PropDef> = new Map(DEFS.map((d) => [d.id, d]));

function assign(props: Properties, name: keyof Properties, value: unknown): void {
  (props as Record<string, unknown>)[name] = value;
}

/** Reads a property bag: its length, then that many bytes of properties. */
export function readProperties(reader: ByteReader): Properties {
  const length = reader.varint();
  const body = reader.section(length);
  const props: Properties = {};
  const seen = new Set<number>();
  while (!body.done) {
    const id = body.varint();
    const def = BY_ID.get(id);
    if (def === undefined) {
      throw new ProtocolError(`unknown MQTT property identifier 0x${id.toString(16)}`);
    }
    if (def.repeat !== true && seen.has(id)) {
      throw new ProtocolError(`duplicate MQTT property 0x${id.toString(16)}`);
    }
    seen.add(id);
    switch (def.type) {
      case "byte":
        assign(props, def.name, body.u8());
        break;
      case "u16":
        assign(props, def.name, body.u16());
        break;
      case "u32":
        assign(props, def.name, body.u32());
        break;
      case "varint": {
        const list = (props.subscriptionIdentifiers ??= []);
        list.push(body.varint());
        break;
      }
      case "utf8":
        assign(props, def.name, body.utf8());
        break;
      case "binary":
        assign(props, def.name, body.binary());
        break;
      case "pair": {
        const key = body.utf8();
        const value = body.utf8();
        const list = (props.userProperties ??= []);
        list.push([key, value]);
        break;
      }
    }
  }
  return props;
}

function writeValue(writer: ByteWriter, type: PropType, value: unknown): void {
  switch (type) {
    case "byte":
      writer.u8(value as number);
      break;
    case "u16":
      writer.u16(value as number);
      break;
    case "u32":
      writer.u32(value as number);
      break;
    case "varint":
      writer.varint(value as number);
      break;
    case "utf8":
      writer.utf8(value as string);
      break;
    case "binary":
      writer.binary(value as Uint8Array);
      break;
    case "pair":
      throw new ProtocolError("a user property is written as a pair, not a value");
  }
}

/**
 * Writes a property bag: the variable byte integer length, then the properties.
 *
 * The body is built first and its length counted, because the length comes
 * before the bytes it measures and cannot be patched afterwards without moving
 * them.
 */
export function writeProperties(writer: ByteWriter, props: Properties | undefined): void {
  const body = new ByteWriter(64);
  if (props !== undefined) {
    for (const def of DEFS) {
      const value = (props as Record<string, unknown>)[def.name];
      if (value === undefined) continue;
      if (def.type === "pair") {
        for (const [key, val] of value as Array<[string, string]>) {
          body.varint(def.id).utf8(key).utf8(val);
        }
        continue;
      }
      if (Array.isArray(value)) {
        for (const item of value) {
          body.varint(def.id);
          writeValue(body, def.type, item);
        }
        continue;
      }
      body.varint(def.id);
      writeValue(body, def.type, value);
    }
  }
  const bytes = body.finish();
  writer.varint(bytes.length).raw(bytes);
}
