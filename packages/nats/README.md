# @opentf/esrun-nats

A NATS client for [esrun](https://esrun.opentechf.org), speaking core NATS,
written entirely in JavaScript over `runtime:net`. There is no native code in
this package, and none was added to the runtime for it.

```sh
npm install @opentf/esrun-nats
```

```js
import { NatsClient } from "@opentf/esrun-nats";

const client = await NatsClient.connect("nats://localhost:4222");

const subscription = client.subscribe("events.>");
for await (const message of subscription) {
  console.log(message.subject, message.json());
}

await client.publish("events.user.created", JSON.stringify({ id: 1 }));
await client.close();
```

Run it with the network grant scoped to the server:

```sh
esrun --allow-net=localhost:4222 app.js
```

The package's exports are the whole public surface; the wire protocol and its
codec stay internal — except the codec itself, which is exported so a test or a
stub server can use it. What it is made of, and why each rule is there, is on the
[internals page](https://esrun.opentechf.org/docs/internals/nats); the decision
that made it a package is `docs/DECISIONS.md` D173. A task-shaped introduction is
the [guide](https://esrun.opentechf.org/docs/guides/nats).

## Connecting

`NatsClient.connect(url, options?)` parses the URL, opens a socket and completes
the handshake, then starts the command pump and keepalive.

```js
const client = await NatsClient.connect("nats://app:secret@nats.internal:4222", {
  name: "worker-1",
});
```

| URL part | |
| --- | --- |
| host, port | The server. The port defaults to 4222 for both schemes. |
| `user:password@` | The credentials, percent-decoded. |
| `token@` (no colon) | A token credential, sent as `auth_token` — which is how NATS tells a token from a user. |
| `nats:` | Plain, unless the server requires otherwise. |
| `tls:` | TLS, negotiated **in band** after `INFO` — see below. |

| Setting | |
| --- | --- |
| `servers` | Extra servers to try, beyond the one the URL named, as `host:port` strings or `{ hostname, port, tls }`. |
| `username`, `password`, `token` | The credentials; override what the URL carries. |
| `name` | A connection name, shown in the server's monitoring. |
| `verbose`, `pedantic` | Ask the server to `+OK` every command, or to validate subjects strictly. Both default `false`. |
| `echo` | Receive this client's own publishes. Defaults `true`. |
| `headers` | Advertise header support when the server offers it. Defaults `true`. |
| `noResponders` | Advertise no-responder statuses when the server offers headers. Defaults `true`. |
| `tls` | Require TLS. |
| `ca`, `servername` | Trust anchors, as PEM, for a private authority; and the name the certificate must carry when it is not the host. |
| `pingInterval` | Milliseconds of silence before a `PING`. Defaults 120 000. |
| `timeout` | Milliseconds the handshake, and the default for a request, may take. Defaults 30 000. |
| `reconnect` | How to redial after a loss: `false` to disable, or `{ maxAttempts, wait, jitter }`. Defaults on (10 attempts, 2 s apart, plus up to 100 ms of jitter — 1 s with TLS). |

`parseNatsUrl(url)` returns the same host and credentials as a plain object,
without dialling: `hostname`, `port`, `tls`, and optionally `username`/`password`/
`token`.

| Client member | |
| --- | --- |
| `connected` | Whether the connection is up (and not mid-reconnect). |
| `closed`, `failure` | Whether it is over, and the error that ended it (`null` while it stands). |
| `onDisconnect`, `onReconnect` | The connection was lost; it is back, with every subscription re-sent. |
| `onError` | News that did not end the client: a recoverable `-ERR` (an invalid subject, a permissions violation) or a slow consumer dropping messages. |
| `serverVersion`, `maxPayload` | What `INFO` advertised: the server's version and its payload ceiling. |
| `publish(subject, payload, options?)` | See below. |
| `subscribe(subject, options?)` | Returns a `Subscription` synchronously. |
| `request(subject, payload, options?)` | One `await` for a reply. |
| `flush(timeout?)` | A `PING`, resolved by its `PONG` — a round trip to the server. Defaults 30 000 ms; rejects `ERR_NATS_TIMEOUT`. |
| `close()` | Sends what is still queued (up to 5 s), then closes the connection, ending every subscription. |
| `connection` | The `Connection` underneath, for a caller that wants the protocol itself. |
| `over(transport, options?)` | Connects over an already-open transport — the seam a test stands on. |

## Publishing and subscribing

A subject is a `.`-separated token path. A subscriber may use `*` for one token
or `>` for the rest, and only `>` may appear last. The server does the matching
and names the subscription in each `MSG`, so delivery is exact.

```js
const subscription = client.subscribe("orders.*.created", { queue: "billing" });

for await (const message of subscription) {
  await handle(message.text());
}
```

Core NATS `publish` is fire-and-forget: it resolves once the bytes are written,
and there is no acknowledgement to await. The one refusal is a message larger
than the server's advertised maximum, which is caught before it is written.

```js
await client.publish("orders.eu.created", JSON.stringify(order));
```

| Publish option | |
| --- | --- |
| `reply` | A subject replies should go to — set by `request` for you. |
| `headers` | A header block. Requires a server that advertised header support. |
| `status` | A `NATS/1.0` status, for a responder answering with one. |

| Subscribe option | |
| --- | --- |
| `queue` | Join a queue group: one member of the group gets each message. |
| `max` | End after this many messages; the server is told (`UNSUB <sid> <max>`), and a reconnect re-sends what is left of it. |
| `pendingLimits` | `{ messages, bytes }` that may wait unread — the Go client's slow-consumer limits, 524 288 and 64 MiB by default. Past them messages are dropped, counted in `dropped`, and reported once through `onError` (`ERR_NATS_SLOW_CONSUMER`). |

| Subscription member | |
| --- | --- |
| `subject`, `queue`, `sid` | What it was made with, and the id the server names. |
| `closed` | Whether it has ended. |
| `dropped` | Messages dropped past `pendingLimits`. |
| `next()`, `nextMessage(timeout?)` | The next message; `nextMessage` rejects with `ERR_NATS_TIMEOUT` if none arrives. |
| `unsubscribe()` | Unsubscribes at the server and ends the iterable. |

A `NatsMessage` is `subject`, `sid`, `reply` (or `null`), `payload`
(`Uint8Array`), `headers` (each name holding every value it carried) and `status`
(a `NATS/1.0` status or `null`), plus `text(encoding?)`, `json()`, `header(name)`
(matched case-insensitively), and `isError` for a status at or above 300.

## Request/reply

Core NATS defines request/reply as a convention: subscribe to an inbox, publish
with it as `reply`, and read the first message back. When the server speaks
headers and no one is listening, it answers with a `503` status instead of
silence, which becomes `ERR_NATS_NO_RESPONDERS` rather than a timeout.

```js
const reply = await client.request("service.fib", "10", { timeout: 5_000 });
console.log(reply.text());
```

The responder is an ordinary subscriber: it reads `message.reply` and publishes
its answer there.

```js
const service = client.subscribe("service.fib");
for await (const request of service) {
  await client.publish(request.reply, String(fib(Number(request.text()))));
}
```

## Reconnection and TLS

A lost socket is not fatal: the connection redials the next server in its pool
(including ones rediscovered from `INFO`'s `connect_urls`), re-runs the handshake,
and re-establishes every subscription under its original id. A `publish` issued
while the connection is down waits for it to come back and is sent **after** the
resubscriptions, so a message cannot reach a subject before its subscription
does. Set `reconnect: false` to fail instead.

TLS is negotiated **after `INFO`**, not at connect — that is how NATS does it. The
server opens in the clear, sends `INFO`, and only then does the client begin the
handshake, which is why `tls://` and `tls: true` are not "TLS from the first
byte". A certificate the client cannot verify fails the connection;
`runtime:net`'s own verifier decides, and there is no option here to switch it
off. A client that asked for TLS uses it with every server, including those
learned from `connect_urls` (which carry no TLS information); a server that
requires TLS the client did not ask for is upgraded to.

## The lower layers

The `Connection` and the transport are exported for a caller that wants to drive
the protocol itself, or stand in for the server. `openConnection(url, settings?,
handlers?)` opens one and reports what needs an application decision through
`ConnectionHandlers` (`onMessage`, `onDisconnect`, `onReconnect`, `onClosed`).
`Connection.over(transport, settings?, handlers?)` takes an already-open
`Transport` instead of dialling; `netTransport(options)` is the one over
`runtime:net`. A stand-in transport implements `read`, `write` and `close`, and
may implement `startTls` for a TLS upgrade; `close` is called with
`{ abort: true }` when the connection is being torn down after a failure, so the
implementation can drop a write still pending rather than wait on it. The wire
codec (`encodeConnect`, `encodePub`, `encodeHpub`, `encodeSub`, `encodeUnsub`,
`encodePing`, `encodePong`, `drain`, `parseInfo`), the subject rules
(`validateSubject`, `validateQueue`, `subjectMatches`, `createInbox`) and the
header block (`encodeHeaders`, `decodeHeaders`) are exported alongside them.

## Errors

Every failure is a `NatsError` (or its `ProtocolError` subclass) with a `code`,
and the server's own `-ERR` text kept verbatim on `serverError`.

```js
import { NatsError, NatsErrorCode } from "@opentf/esrun-nats";

try {
  await client.request("service.fib", "10");
} catch (e) {
  if (e instanceof NatsError && e.code === NatsErrorCode.NoResponders) fallback();
  else throw e;
}
```

| Code | |
| --- | --- |
| `ERR_NATS_PROTOCOL` | The bytes were not NATS — this client's or the server's framing disagreeing. |
| `ERR_NATS_CONNECTION` | The socket could not be opened, was lost, or a command could not be read. |
| `ERR_NATS_CLOSED` | The connection was closed, by this client or the server. |
| `ERR_NATS_TIMEOUT` | Something did not answer within the time allowed. |
| `ERR_NATS_HANDSHAKE` | The handshake failed, before or instead of the `INFO`/`CONNECT` exchange. |
| `ERR_NATS_AUTH` | The server refused the credentials. |
| `ERR_NATS_SERVER` | The server sent a `-ERR`, or refused a command. |
| `ERR_NATS_NO_RESPONDERS` | A request found no responder, and the server said so. |
| `ERR_NATS_PAYLOAD` | A message is larger than the server's advertised maximum. |
| `ERR_NATS_SLOW_CONSUMER` | A subscription's pending limits were full and messages were dropped (via `onError`). |
| `ERR_NATS_UNSUPPORTED` | Something this client does not implement was asked for. |

Bytes that are not NATS are fatal and latched: the stream is at an unknown
boundary, so the connection does not try to resynchronise it. A lost *socket*, by
contrast, is recovered by reconnecting, as above.

A server `-ERR` is handled as the protocol's table says. *Invalid Subject* and a
*Permissions Violation* leave the connection open: they reach `onError`, and a
refused subscription ends with the server's words on `serverError`. Every other
`-ERR` (*Stale Connection*, *Slow Consumer*, *Maximum Connections Exceeded*…) is
followed by the server closing the socket, and is recovered by reconnecting.

## Testing locally

The harness starts a NATS for you — a plain one on 4222, and a TLS one on 4223
with a private certificate authority:

```sh
eval "$(test/nats-server.sh)"   # nats:2, plain and TLS
```

Then `NatsClient.connect("nats://127.0.0.1:4222")` — or point `test/run.sh` at a
server of your own through `NATS_URL` and it runs the end-to-end suite against
it.

## Not yet

JetStream (persistence, streams and consumers), the key-value and object stores,
the services framework, and nkey/JWT/`.creds` authentication. This is a core NATS
client: publish/subscribe, queue groups, request/reply, headers and reconnection.
