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

- A 3.1.1 CONNACK refusal (return codes 1–5) was read by the 5.0 rule (refusal
  from 0x80) and accepted as a successful connection; a bad password surfaced
  only as a later "broker closed the connection". Codes 4 and 5 are now
  `ERR_MQTT_AUTH`, 1 `ERR_MQTT_HANDSHAKE`, the rest `ERR_MQTT_REFUSED`.
- The broker's CONNACK limits (5.0) are kept: a QoS 1/2 publish beyond its
  Receive Maximum waits for a slot within its timeout, and a publish above its
  Maximum QoS, a retained one without Retain Available, a packet over its
  Maximum Packet Size, and a wildcard, shared or identified subscription it
  lacks reject with `ERR_MQTT_UNSUPPORTED` instead of getting the client
  disconnected.
- Messages that arrived before the client existed — a resumed session's backlog,
  sent right after the CONNACK and already acknowledged — were dropped. They
  are held, and messages no subscription matches go to the new
  `onUnmatchedMessage` option.
- `request` used a `$`-prefixed reply topic, which the specification reserves
  for the broker, and `Math.random` for its token and correlation data. It now
  uses `esrun/reply/<token>` and `crypto.getRandomValues` (16 bytes).
- A packet identifier still awaiting its acknowledgement could be reused when
  the counter wrapped.
- Unsubscribing one of two subscriptions to the same filter ended both; the
  UNSUBSCRIBE is now sent only for the last.
- Integer fields (`keepAlive`, intervals, identifiers) out of range wrapped
  silently; they are refused.
- Receiving a large packet in many chunks copied the whole buffer per chunk.
- Inbound Topic Aliases (5.0) are resolved when `topicAliasMaximum` is
  advertised, instead of failing on the empty topic.

- Concurrent `ping` calls no longer share one outstanding flag. A PINGRESP
  carries no identifier, so two exchanges could not be told apart — one reply
  answered every caller, and a reply to a ping already given up on answered the
  next one. There is now a single in-flight exchange that concurrent callers
  join, its timeout is treated as a lost connection rather than a stray reply,
  and keep-alive waits for it rather than sending a second PINGREQ.
- A `publish` whose packet cannot be encoded no longer leaves its acknowledgement
  waiter registered. This failure is local — the packet never reaches the socket,
  so the connection stays up and its own cleanup never runs — and the abandoned
  waiter's timer would reject with nobody listening.
- A waiter's timeout no longer rejects an unawaited promise when a write outlasts
  it. A `publish`, `subscribe` or `unsubscribe` arms its timer before `send`, and
  awaits the waiter only after `send` resolves; a write slower than the timeout —
  writes are serialised, so this happens — fired the timer while the caller was
  still awaiting the socket. The waiter now carries a handler from creation, so
  that rejection waits for the caller's `await` instead of becoming unhandled.
- A `publish`, `subscribe` or `unsubscribe` whose write stalls past the timeout
  now rejects at the timeout, rather than waiting for a write that may never
  finish. The write and the acknowledgement are awaited together, so whichever
  fails first settles the call.
- A write that outlives the acknowledgement timeout now fails the connection. The
  packet may already be with the transport, and the stuck write still held the
  serialised chain — blocking every later packet. Failing closes the socket,
  releases the queue and prevents later packets from being sent behind it.
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
- A failure that tears the connection down no longer leaves the socket open. The
  socket transport closes the writer before the socket — a tidy half-close — but
  the writer's close waits for a write the peer has stopped reading, so the socket
  close that would interrupt it was never reached and the connection reported
  itself closed over a still-open socket. The failure path now closes the socket
  directly, which interrupts a pending write instead of waiting on it.
- A deliberate `close()` during a pending write no longer waits for that write to
  finish. Since a DISCONNECT would be queued behind it, the connection aborts the
  transport directly, settles the operation with `Closed`, and leaves `failure`
  null. When no write is pending, close still sends DISCONNECT before closing.
