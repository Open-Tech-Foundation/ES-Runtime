// The MQTT 5.0 property bag. The interesting cases are the two repeatable
// properties, the duplicate rule for the rest, and a length prefix that
// disagrees with the bytes that follow it.
import { exit } from "runtime:process";
import { readProperties, writeProperties } from "../../dist/protocol/properties.js";
import { ByteReader, ByteWriter } from "../../dist/protocol/wire.js";
import { ok, report, throws } from "./assert.mjs";

function roundTrip(props) {
  const writer = new ByteWriter(64);
  writeProperties(writer, props);
  return readProperties(new ByteReader(writer.finish()));
}

const props = {
  payloadFormatIndicator: 1,
  messageExpiryInterval: 60,
  contentType: "application/json",
  responseTopic: "reply/here",
  correlationData: new Uint8Array([1, 2, 3]),
  subscriptionIdentifiers: [1, 300],
  sessionExpiryInterval: 120,
  assignedClientIdentifier: "assigned-1",
  serverKeepAlive: 30,
  authenticationMethod: "SCRAM-SHA-1",
  authenticationData: new Uint8Array([9, 8]),
  requestProblemInformation: 0,
  willDelayInterval: 5,
  requestResponseInformation: 1,
  responseInformation: "resp",
  serverReference: "other.example:1883",
  reasonString: "because",
  receiveMaximum: 10,
  topicAliasMaximum: 5,
  topicAlias: 2,
  maximumQoS: 1,
  retainAvailable: 1,
  userProperties: [
    ["a", "1"],
    ["b", "2"],
  ],
  maximumPacketSize: 1024,
  wildcardSubscriptionAvailable: 1,
  subscriptionIdentifierAvailable: 0,
  sharedSubscriptionAvailable: 1,
};

const read = roundTrip(props);
ok(read.payloadFormatIndicator === 1, "a byte property round-trips");
ok(read.messageExpiryInterval === 60, "a four-octet integer round-trips");
ok(read.contentType === "application/json", "a UTF-8 string round-trips");
ok(read.responseTopic === "reply/here", "a response topic round-trips");
ok(Array.from(read.correlationData).join() === "1,2,3", "binary data round-trips");
ok(read.sessionExpiryInterval === 120, "session expiry round-trips");
ok(read.assignedClientIdentifier === "assigned-1", "the assigned identifier round-trips");
ok(read.serverKeepAlive === 30, "server keep-alive round-trips");
ok(read.authenticationMethod === "SCRAM-SHA-1", "an authentication method round-trips");
ok(Array.from(read.authenticationData).join() === "9,8", "authentication data round-trips");
ok(read.requestProblemInformation === 0, "a zero-valued byte survives (not treated as absent)");
ok(read.willDelayInterval === 5, "will delay round-trips");
ok(read.responseInformation === "resp", "response information round-trips");
ok(read.serverReference === "other.example:1883", "a server reference round-trips");
ok(read.reasonString === "because", "a reason string round-trips");
ok(read.receiveMaximum === 10, "receive maximum round-trips");
ok(read.topicAliasMaximum === 5, "topic alias maximum round-trips");
ok(read.topicAlias === 2, "a topic alias round-trips");
ok(read.maximumQoS === 1, "maximum QoS round-trips");
ok(read.maximumPacketSize === 1024, "maximum packet size round-trips");
ok(read.subscriptionIdentifierAvailable === 0, "a zero availability flag survives");

// Repeatable properties keep every occurrence, in order.
ok(read.userProperties.length === 2, "both user properties are kept");
ok(read.userProperties[0][0] === "a" && read.userProperties[1][1] === "2", "in order");
ok(
  read.subscriptionIdentifiers.join() === "1,300",
  "both subscription identifiers are kept, in order",
);

// An empty bag is a length of zero and no bytes.
const empty = roundTrip({});
ok(Object.keys(empty).length === 0, "an empty bag decodes to nothing");
const noBag = roundTrip(undefined);
ok(Object.keys(noBag).length === 0, "an absent bag decodes to nothing");

// A duplicate non-repeatable property is refused.
await throws(
  () =>
    readProperties(new ByteReader(new Uint8Array([0x0a, 0x02, 0, 0, 0, 60, 0x02, 0, 0, 0, 61]))),
  "a duplicated message-expiry interval is refused",
);

// An unknown identifier is refused.
await throws(
  () => readProperties(new ByteReader(new Uint8Array([0x02, 0x7f, 0x00]))),
  "an unknown property identifier is refused",
);

// A length prefix larger than the bytes that follow is refused.
await throws(
  () => readProperties(new ByteReader(new Uint8Array([0x08, 0x02, 0x00]))),
  "a property length past the end of the buffer is refused",
);

exit(report("properties") > 0 ? 1 : 0);
