# Changelog for `@opentf/esrun-mqtt`

All notable changes to **`@opentf/esrun-mqtt`**, the MQTT client for ES Runtime,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

### Added

- The `@opentf/esrun-mqtt` package, an MQTT client speaking MQTT 5.0 with a
  fallback to 3.1.1, in JavaScript over `runtime:net` (DECISIONS D172). No native
  code, and no capability beyond the `Net` grant `runtime:net`'s `connect`
  already needs.
- The MQTT wire primitives: the variable byte integer, the UTF-8 string and
  binary data forms, and the reader and writer every packet is built from.
  Bounds-checked on every field, so a short packet is a protocol error and never
  a read past the end of the buffer.
- The MQTT 5.0 property codec: the property identifiers this client needs, the
  variable header they are carried in, and the rule that a value's form follows
  its identifier, so a malformed property is caught rather than misread.
- Topic validation and filter matching: the rule that a publish topic carries no
  wildcard and a filter may, the `+` and `#` wildcards and where `#` may appear,
  and the empty-and-oversized cases the specification refuses.
- The packet codec: every control packet — CONNECT, CONNACK, PUBLISH, the four
  acknowledgements, SUBSCRIBE/SUBACK, UNSUBSCRIBE/UNSUBACK, PINGREQ/PINGRESP,
  DISCONNECT and AUTH — encoded and decoded for both MQTT 5.0 and 3.1.1, with
  the fixed-header flags enforced and the packets that carry neither properties
  nor reason codes read the way the older level means them.
- The connection layer: the CONNECT/CONNACK handshake, the version negotiation
  that offers 5.0 and falls back to 3.1.1 when a broker will not speak it, the
  packet pump that reassembles packets from whatever the socket delivered, the
  protocol's own acknowledgements (PUBACK for an inbound QoS 1 message, and the
  PUBREC/PUBREL/PUBCOMP exchange in either direction, so a QoS 2 message is
  delivered once, on PUBREL), keep-alive with PINGREQ, and a deliberate
  DISCONNECT. A lost socket or bytes that are not MQTT is latched once so every
  later caller sees the one cause.
- A `Transport` seam with a `runtime:net` implementation, so the connection and
  the client can be exercised against a scripted broker with no port open and no
  capability granted.
- The client layer: publishing at QoS 0, 1 and 2 with the QoS 1 and 2 calls
  resolving only on the broker's acknowledgement (or refusing with its reason
  code), subscribing that returns an async iterable of messages routed by the
  topic filter, unsubscribe, and request/reply over the MQTT 5.0
  response-topic/correlation-data convention. `mqtt://` reaches 1883 and
  `mqtts://` reaches 8883 over TLS.
- The `Will` type — the shape a connection's `will` setting names — is exported,
  so a last will can be built and named rather than only written inline.

### Fixed

- A `Subscription.nextMessage` timeout no longer leaves its waiter queued. The
  waiter is now withdrawn, so a later delivery resolves the next read rather than
  the abandoned one — which had quietly dropped that message.
- The handshake timeout now races the socket read, not only checks it between
  reads. A broker that accepts the connection and then sends nothing fails at the
  timeout instead of leaving `connect` pending indefinitely.
- A negative PUBREC now fails a QoS 2 publish at once, carrying the broker's
  reason code, instead of sending PUBREL and waiting out a PUBCOMP that never
  comes. Per the specification, no PUBREL is sent and the packet identifier is
  free to reuse immediately.
- `ping` is resolved only by a PINGRESP. Unrelated inbound traffic no longer
  clears the outstanding ping and lets the call return early.
- A `subscribe` that times out — or whose SUBSCRIBE never went out — no longer
  leaves the subscription registered, where it would buffer matching messages
  with no one left to read them. The same cleanup applies to `unsubscribe`.
- `Connection.closed` now reports a deliberate `close()`, not only a failure.
