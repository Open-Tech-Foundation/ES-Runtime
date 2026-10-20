# Changelog for `@opentf/esrun-nats`

All notable changes to **`@opentf/esrun-nats`**, the NATS client for ES Runtime,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

### Added

- `onDisconnect`, `onReconnect` and `onError` on the client; the `max` and
  `pendingLimits` subscribe options and `Subscription.dropped`;
  `ERR_NATS_SLOW_CONSUMER`; reconnect jitter (DECISIONS D179).
- The `@opentf/esrun-nats` package, a client speaking core NATS, in JavaScript
  over `runtime:net` (DECISIONS D173). No native code, and no capability beyond
  the `Net` grant `runtime:net`'s `connect` already needs.
- The NATS wire codec: the `INFO`/`CONNECT`/`PUB`/`HPUB`/`SUB`/`UNSUB`/`PING`/
  `PONG`/`-ERR` commands encoded and decoded, and a streaming `drain` that
  reassembles them from whatever the socket delivered and validates every
  byte-counted body, so a short payload is a protocol error and never a read past
  the end of the buffer.
- Subject validation and matching: the rule that a publish subject carries no
  wildcard, that `*` matches one token and `>` the rest and only as the last, the
  `$`-reserved prefix, and the empty-and-oversized cases core NATS refuses.
- The `NATS/1.0` header block: encode and decode of the `HPUB`/`HMSG` header
  section and its status line, rejecting a header that would inject a line
  break.
- Inbox generation: `createInbox()` returns a `_INBOX.` subject with a
  collision-resistant token, for request/reply.
- A `Transport` seam with a `runtime:net` implementation, and an in-band TLS
  upgrade (`startTls`) — NATS opens in the clear, sends `INFO`, and only then
  begins the handshake — so the connection and client can be exercised against a
  scripted server with no port open and no capability granted.
- The connection layer: the `INFO`/`CONNECT` handshake with `PING`/`PONG` as the
  readiness step, the command pump that reassembles commands across reads, the
  protocol's own chatter (a server `PING` answered, this client's `PONG`s matched
  to its `PING`s in order), keepalive with a staleness window, and a deliberate
  close. A `-ERR` is surfaced with the server's own text, and a lost socket or
  bytes that are not NATS is latched once so every later caller sees the one
  cause.
- Automatic reconnection over a server pool: a lost socket redials the next
  server (including ones rediscovered from `INFO`'s `connect_urls`), re-runs the
  handshake, and re-establishes every subscription under its original id. A write
  issued while the connection is down waits for it to come back, and is sent
  after the resubscriptions rather than ahead of them.
- The client layer: `publish` resolving once the bytes are written (core NATS has
  no acknowledgement, and a message larger than the server's advertised maximum
  is refused before it is written), `subscribe` returning an async iterable of
  messages routed by the id the server names (so delivery is exact and needs no
  wildcard matching on this side) with optional queue groups, request/reply over
  a generated inbox that turns the server's `503` no-responders status into a
  clear error, and `flush`. `nats://` reaches 4222 and `tls://` reaches 4222 over
  TLS.

### Fixed

- Every server `-ERR` ended the client for good. Recoverable ones (an invalid
  subject, a permissions violation) are now reported through `onError` and the
  client carries on; the rest (*Stale Connection*, *Slow Consumer*…) are
  recovered by reconnecting, as the protocol's table says.
- A `tls://` client could reconnect in the clear to a server learned from
  `connect_urls`. Discovered servers now inherit TLS, and a server that requires
  TLS is upgraded to instead of refused.
- `close()` aborted queued writes, so publishes that were not awaited were never
  sent (0 of 2 000 against a real server). It now flushes, for up to 5 s.
- Each subscription buffered without bound; it now has the Go client's pending
  limits.
- Receiving a large message in many chunks copied the whole buffer per chunk.
- An IPv6 entry in `connect_urls` kept its brackets and could not be dialled.
- An auto-unsubscribe limit was lost on a reconnect.
- Inbox tokens came from `Math.random`; they come from the CSPRNG.
- A `flush` that times out no longer lets its late `PONG` settle a later flush.
  A `PONG` carries no identifier and the server answers in order, so a timed-out
  caller now keeps its place in the queue — the `PONG` it is owed is consumed by
  it rather than by the next flush, which would otherwise report success one
  round trip early.
- A `publish` waiting for a reconnect is no longer left hanging when the
  connection is closed. The readiness gate a down write parks on is rejected by
  `close`, so the write is settled with `ERR_NATS_CLOSED` instead of waiting
  behind a reconnect that is never coming.
- A header status description containing CR or LF is refused, as header names and
  values already were. It would otherwise inject a line into the encoded header
  block.
