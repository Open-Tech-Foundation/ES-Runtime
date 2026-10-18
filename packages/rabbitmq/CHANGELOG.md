# Changelog for `@opentf/esrun-rabbitmq`

All notable changes to **`@opentf/esrun-rabbitmq`**, the RabbitMQ (AMQP 0-9-1)
client for ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

### Added

- The `@opentf/esrun-rabbitmq` package, a RabbitMQ client speaking AMQP 0-9-1 in
  JavaScript over `runtime:net` (DECISIONS D171). No native code, and no
  capability beyond the `Net` grant `runtime:net`'s `connect` already needs.
- The package's public surface: the connection, channel, consumer, request/reply,
  and pool layers re-exported from the package root and nothing else. The wire
  protocol and its codecs stay internal, so only the root is reachable.
- The AMQP 0-9-1 wire codec: the protocol header and method, content-header,
  body, and heartbeat frames, decoded by a resumable frame decoder; RabbitMQ's
  field-table tags; and the `basic` class's content header with its property
  flags. Unit-tested without a broker.
- The method table: the classes, methods, and arguments this client speaks,
  with one encoder and one decoder over it. Adjacent bit fields share an octet
  and reserved fields carry the specification's defaults, so a caller writes
  `basic.publish` with an exchange and a routing key and nothing else.
- The connection layer: the AMQP handshake (protocol header, `start`/`start-ok`
  with SASL PLAIN, the tuning negotiation and its zero-means-no-proposal rule,
  `open`), the frame pump that routes methods, headers, and bodies to their
  channels, channel bookkeeping, heartbeats, and a deliberate close. Fatal
  failures — a lost socket, a frame that is not AMQP — are latched once so every
  later caller sees the one cause rather than a new symptom.
- A `Transport` seam with a `runtime:net` implementation, so the connection can
  be exercised against a scripted in-memory broker with no port open and no
  capability granted.
- The channel layer: exchange and queue declaration, deletion, binding, and
  purging; `basic.qos` prefetch; publishing with the `basic` properties and a
  body split across frames; `basic.get`; acknowledgements, nacks, rejects, and
  `recover`; and publisher confirms, where an awaited publish resolves only once
  the broker has taken the message and rejects when it nacks. A delivery's
  method, header, and body frames are reassembled into one message, and a
  mandatory publish that cannot be routed reaches `onReturn`.
- The consumer layer: `subscribe` opens a subscription with a generated consumer
  tag and returns an async iterable of deliveries; each delivery carries its own
  `ack`, `nack`, and `reject`, and on a `noAck` subscription those refuse with a
  channel error rather than send a frame the broker would treat as a protocol
  violation. Deliveries buffer until they are read, `break` cancels the
  consumer, and a broker-initiated cancellation ends the iterator with a
  `Cancellation` naming the broker as the cause.
- The RPC layer: an `RpcClient` over a channel consumes `amq.rabbitmq.reply-to`
  with `noAck` and answers `call`s, sending each request a generated correlation
  id and the direct reply-to address; replies are matched by id, and out-of-order
  or unknown ones are dropped. A call left unanswered past its timeout fails
  with `ERR_RABBITMQ_TIMEOUT`, a call in flight when the channel closes fails
  with the channel's reason, and `close` cancels the reply consumer and fails
  whatever is still pending.
- The pool: a `Pool` over one broker URL lends at most `size` connections, reuses
  an idle one before dialling, and drops a borrowed connection the moment it
  stops being usable so the next borrow dials a replacement — which is the whole
  of its recovery. `withConnection` and `withChannel` scope a lease; `onConnect`
  runs for each connection the pool opens, where an application re-declares what
  a lost connection had. `parseAmqpUrl` and `openConnection` read an `amqp://` or
  `amqps://` URL and are the one home for the address it names.
- An end-to-end test against a real RabbitMQ (`test/e2e.mjs`, started by
  `test/rabbitmq-server.sh`): the handshake and negotiated tuning, a refused
  password, confirmed publishes and a prefetched subscription that acknowledges
  in order with every property round-tripping, `basic.get`, an exchange and a
  binding, a returned mandatory publish, an RPC exchange over
  `amq.rabbitmq.reply-to` with a concurrent call and a timeout, a pool that opens
  two connections under concurrent borrows, and an exclusive queue that goes with
  its connection.

### Documentation

- The package README is now the API reference: connecting and the URL form, the
  connection and channel members, topology, publishing and confirms, consuming,
  request/reply, the pool, every option with its default, and the error codes.
- A task-shaped [guide](/docs/guides/rabbitmq) and an
  [Internals page](/docs/internals/rabbitmq) on the site, the latter covering why
  writes are serialised, why a lost socket is fatal, and what the pool's recovery
  does and does not do.

### Fixed

- An `amqps://` URL without a port defaulted to 5672, so a standard TLS URL
  reached the plain AMQP listener and tried to negotiate TLS there. The default
  now follows the scheme: 5672 for `amqp:`, 5671 for `amqps:`.
- A password refused at the SASL step is reported as `ERR_RABBITMQ_AUTH`, not
  `ERR_RABBITMQ_SERVER`. RabbitMQ refuses a bad password by closing the
  connection in place of `connection.tune`, before the handshake had ever
  switched its refusal to an auth failure; the mapping now covers the whole
  handshake, not just `connection.open-ok`.
- Two publishes issued at once could interleave their method, header, and
  content frames, which RabbitMQ refuses with a 505 `UNEXPECTED_FRAME`. A
  connection's writes — the protocol header excepted — now share one queue, so a
  content frame-set is written whole.
