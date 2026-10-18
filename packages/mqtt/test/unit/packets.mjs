// The packet codec, both protocol levels. Each packet is encoded and read back,
// and the reserved bits, the QoS/dup rules, and the "too many bytes" cases are
// pinned as errors.
import { exit } from "runtime:process";
import { decodePacket, encodePacket, PacketType } from "../../dist/protocol/packets.js";
import { ok, report, throws } from "./assert.mjs";

const DECODER = new TextDecoder();
const ENCODER = new TextEncoder();

function same(a, b) {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

function bytesOf(packet, level = 5) {
  return Array.from(encodePacket(packet, level));
}

function firstByte(packet, level = 5) {
  return encodePacket(packet, level)[0];
}

// --- CONNECT ---------------------------------------------------------------

const connect = {
  type: PacketType.Connect,
  clientId: "client-1",
  username: "user",
  password: new Uint8Array([1, 2, 3]),
  keepAlive: 60,
  cleanStart: true,
  will: {
    topic: "will/topic",
    payload: new Uint8Array([9, 9]),
    qos: 1,
    retain: true,
    properties: { willDelayInterval: 3 },
  },
  properties: { sessionExpiryInterval: 300, receiveMaximum: 20 },
};

ok(firstByte(connect) === 0x10, "CONNECT's first octet is 0x10");
const connectRead = decodePacket(encodePacket(connect, 5));
ok(connectRead.type === PacketType.Connect, "a CONNECT decodes as a CONNECT");
ok(connectRead.clientId === "client-1", "the client identifier round-trips");
ok(connectRead.username === "user", "the username round-trips");
ok(Array.from(connectRead.password).join() === "1,2,3", "the password round-trips as bytes");
ok(connectRead.keepAlive === 60, "keep-alive round-trips");
ok(connectRead.cleanStart === true, "clean start round-trips");
ok(connectRead.will.topic === "will/topic", "the will topic round-trips");
ok(
  connectRead.will.qos === 1 && connectRead.will.retain === true,
  "the will QoS and retain round-trip",
);
ok(connectRead.will.properties.willDelayInterval === 3, "the will's own properties round-trip");
ok(connectRead.properties.sessionExpiryInterval === 300, "the CONNECT properties round-trip");

// A 3.1.1 CONNECT has no properties, and the packet states its own level.
const connect4 = decodePacket(encodePacket(connect, 4));
ok(connect4.username === "user", "a 3.1.1 CONNECT decodes its payload");
ok(connect4.properties === undefined, "a 3.1.1 CONNECT carries no properties");
ok(connect4.will.properties === undefined, "a 3.1.1 will carries no properties");
ok(connect4.will.payload.length === 2, "a 3.1.1 will payload decodes");

// --- CONNACK ---------------------------------------------------------------

const connack = decodePacket(
  encodePacket(
    {
      type: PacketType.Connack,
      sessionPresent: true,
      reasonCode: 0,
      properties: { assignedClientIdentifier: "assigned", receiveMaximum: 10 },
    },
    5,
  ),
);
ok(connack.sessionPresent === true, "the CONNACK session-present flag round-trips");
ok(connack.reasonCode === 0, "the CONNACK reason code round-trips");
ok(connack.properties.assignedClientIdentifier === "assigned", "CONNACK properties round-trip");

const connack4 = decodePacket(
  encodePacket({ type: PacketType.Connack, sessionPresent: false, reasonCode: 5 }, 4),
  4,
);
ok(connack4.reasonCode === 5, "a 3.1.1 CONNACK return code round-trips");
ok(connack4.properties === undefined, "a 3.1.1 CONNACK carries no properties");

// --- PUBLISH ---------------------------------------------------------------

const publish = {
  type: PacketType.Publish,
  topic: "sensor/temp",
  payload: ENCODER.encode("21.5"),
  qos: 1,
  retain: true,
  dup: true,
  packetId: 7,
  properties: { contentType: "text/plain", userProperties: [["unit", "C"]] },
};
ok(firstByte(publish) === 0x3b, "PUBLISH QoS 1, dup and retain set the low nibble to 0xb");
const publishRead = decodePacket(encodePacket(publish, 5));
ok(publishRead.qos === 1, "the QoS round-trips");
ok(publishRead.dup === true && publishRead.retain === true, "dup and retain round-trip");
ok(publishRead.packetId === 7, "the packet identifier round-trips");
ok(DECODER.decode(publishRead.payload) === "21.5", "the payload round-trips");
ok(publishRead.properties.userProperties[0][1] === "C", "PUBLISH properties round-trip");

const publish0 = decodePacket(
  encodePacket({
    type: PacketType.Publish,
    topic: "a/b",
    payload: new Uint8Array([1]),
    qos: 0,
    retain: false,
    dup: false,
  }),
);
ok(
  publish0.qos === 0 && publish0.packetId === undefined,
  "a QoS 0 PUBLISH has no packet identifier",
);
ok(firstByte({ ...publish0, properties: undefined }) === 0x30, "a QoS 0 PUBLISH's low nibble is 0");

// --- acknowledgements ------------------------------------------------------

const puback = decodePacket(
  encodePacket(
    { type: PacketType.Puback, packetId: 5, reasonCode: 0x10, properties: { reasonString: "nah" } },
    5,
  ),
);
ok(puback.packetId === 5 && puback.reasonCode === 0x10, "PUBACK round-trips reason and id");
ok(puback.properties.reasonString === "nah", "PUBACK properties round-trip");

// The short form: only the packet identifier, reason code implied 0.
const shortAck = decodePacket(new Uint8Array([0x40, 0x02, 0x00, 0x05]), 5);
ok(shortAck.packetId === 5 && shortAck.reasonCode === 0, "a two-byte PUBACK means Success");

for (const type of [PacketType.Pubrec, PacketType.Pubrel, PacketType.Pubcomp]) {
  const read = decodePacket(encodePacket({ type, packetId: 1, reasonCode: 0 }, 5));
  ok(read.type === type && read.packetId === 1, `packet ${type} round-trips`);
}
ok(
  (firstByte({ type: PacketType.Pubrel, packetId: 1, reasonCode: 0 }) & 0x0f) === 2,
  "PUBREL flags are 0b0010",
);

// --- SUBSCRIBE / SUBACK ----------------------------------------------------

const subscribe = decodePacket(
  encodePacket(
    {
      type: PacketType.Subscribe,
      packetId: 9,
      properties: { subscriptionIdentifiers: [7] },
      subscriptions: [
        { filter: "a/+", qos: 1, noLocal: true, retainAsPublished: true, retainHandling: 2 },
        { filter: "b/#", qos: 2 },
      ],
    },
    5,
  ),
);
ok(subscribe.packetId === 9, "the SUBSCRIBE packet identifier round-trips");
ok(subscribe.subscriptions.length === 2, "both filters round-trip");
ok(subscribe.subscriptions[0].noLocal === true, "no-local round-trips");
ok(subscribe.subscriptions[0].retainAsPublished === true, "retain-as-published round-trips");
ok(subscribe.subscriptions[0].retainHandling === 2, "retain-handling round-trips");
ok(subscribe.subscriptions[1].qos === 2, "the second filter's QoS round-trips");
ok(
  subscribe.properties.subscriptionIdentifiers[0] === 7,
  "the subscription identifier round-trips",
);

const suback = decodePacket(
  encodePacket({ type: PacketType.Suback, packetId: 9, reasons: [1, 0x80] }, 5),
);
ok(suback.reasons.join() === "1,128", "SUBACK reason codes round-trip, refusals included");

// --- UNSUBSCRIBE / UNSUBACK ------------------------------------------------

const unsubscribe = decodePacket(
  encodePacket({ type: PacketType.Unsubscribe, packetId: 3, filters: ["a/+", "b/#"] }, 5),
);
ok(unsubscribe.filters.join() === "a/+,b/#", "UNSUBSCRIBE filters round-trip");

const unsuback = decodePacket(
  encodePacket({ type: PacketType.Unsuback, packetId: 3, reasons: [0, 0x11] }, 5),
);
ok(unsuback.reasons.join() === "0,17", "UNSUBACK reason codes round-trip");
const unsuback4 = decodePacket(
  encodePacket({ type: PacketType.Unsuback, packetId: 3, reasons: [] }, 4),
  4,
);
ok(unsuback4.reasons.length === 0, "a 3.1.1 UNSUBACK has only an identifier");

// --- PING / DISCONNECT / AUTH ----------------------------------------------

ok(firstByte({ type: PacketType.Pingreq }) === 0xc0, "PINGREQ's first octet is 0xc0");
ok(firstByte({ type: PacketType.Pingresp }) === 0xd0, "PINGRESP's first octet is 0xd0");
ok(bytesOf({ type: PacketType.Pingreq }).join() === "192,0", "PINGREQ is two bytes");

const disconnect = decodePacket(
  encodePacket(
    { type: PacketType.Disconnect, reasonCode: 0x8e, properties: { sessionExpiryInterval: 0 } },
    5,
  ),
);
ok(disconnect.reasonCode === 0x8e, "a DISCONNECT reason code round-trips");
ok(disconnect.properties.sessionExpiryInterval === 0, "DISCONNECT properties round-trip");
ok(
  bytesOf({ type: PacketType.Disconnect, reasonCode: 0 }, 4).join() === "224,0",
  "a 3.1.1 DISCONNECT is empty",
);

const auth = decodePacket(
  encodePacket(
    { type: PacketType.Auth, reasonCode: 0x18, properties: { authenticationMethod: "x" } },
    5,
  ),
);
ok(auth.reasonCode === 0x18 && auth.properties.authenticationMethod === "x", "AUTH round-trips");

// --- refusals --------------------------------------------------------------

await throws(
  () =>
    encodePacket({
      type: PacketType.Publish,
      topic: "a",
      payload: new Uint8Array(),
      qos: 0,
      retain: false,
      dup: true,
    }),
  "a QoS 0 PUBLISH with DUP set is refused",
);
await throws(
  () =>
    encodePacket({
      type: PacketType.Publish,
      topic: "a/+",
      payload: new Uint8Array(),
      qos: 0,
      retain: false,
      dup: false,
    }),
  "a PUBLISH to a wildcard topic is refused",
);
await throws(
  () =>
    encodePacket({
      type: PacketType.Publish,
      topic: "a",
      payload: new Uint8Array(),
      qos: 1,
      retain: false,
      dup: false,
    }),
  "a QoS 1 PUBLISH with no packet identifier is refused",
);
await throws(
  () => encodePacket({ type: PacketType.Subscribe, packetId: 1, subscriptions: [] }),
  "a SUBSCRIBE with no filter is refused",
);
await throws(
  () => encodePacket({ type: PacketType.Unsubscribe, packetId: 1, filters: [] }),
  "an UNSUBSCRIBE with no filter is refused",
);

// A malformed PUBLISH: QoS 3 in the flags.
await throws(
  () => decodePacket(new Uint8Array([0x36, 0x03, 0x00, 0x01, 0x61]), 5),
  "a PUBLISH QoS 3 is refused",
);
// A DUP on QoS 0.
await throws(
  () => decodePacket(new Uint8Array([0x38, 0x03, 0x00, 0x01, 0x61]), 5),
  "a DUP on a QoS 0 PUBLISH is refused",
);
// PUBREL with wrong flags.
await throws(
  () => decodePacket(new Uint8Array([0x60, 0x02, 0x00, 0x01]), 5),
  "PUBREL flags other than 0b0010 are refused",
);
// An unknown packet type (0).
await throws(() => decodePacket(new Uint8Array([0x00, 0x00]), 5), "packet type 0 is refused");
// Trailing bytes after a complete packet.
await throws(
  () => decodePacket(new Uint8Array([0xc0, 0x00, 0x01]), 5),
  "trailing bytes are refused",
);
// A CONNECT with the reserved flag set.
await throws(
  () =>
    decodePacket(
      new Uint8Array([
        0x10, 0x0d, 0x00, 0x04, 0x4d, 0x51, 0x54, 0x54, 0x05, 0x01, 0x00, 0x3c, 0x00, 0x00, 0x00,
      ]),
      5,
    ),
  "a CONNECT with the reserved flag set is refused",
);

ok(same(new Uint8Array([1, 2]), new Uint8Array([1, 2])), "sanity: same() compares byte arrays");

exit(report("packets") > 0 ? 1 : 0);
