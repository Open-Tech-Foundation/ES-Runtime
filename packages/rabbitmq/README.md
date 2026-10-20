# @opentf/esrun-rabbitmq

A RabbitMQ client for [esrun](https://esrun.opentechf.org), speaking AMQP 0-9-1
and written entirely in JavaScript over `runtime:net`. There is no native code
in this package, and none was added to the runtime for it.

```sh
npm install @opentf/esrun-rabbitmq
```

```js
import { openConnection, subscribe } from "@opentf/esrun-rabbitmq";

const connection = await openConnection("amqp://guest:guest@localhost/");
const channel = await connection.channel();

await channel.queueDeclare("jobs", { durable: true });
await channel.publish("", "jobs", "hello");

for await (const delivery of await subscribe(channel, "jobs")) {
  console.log(new TextDecoder().decode(delivery.body));
  await delivery.ack();
}

await connection.close();
```

Run it with the network grant scoped to the broker:

```sh
esrun --allow-net=rabbitmq.example.com app.js
```

The package's exports are the whole public surface; the wire protocol and its
codecs stay internal. What it is made of, and why each rule is there, is on the
[internals page](https://esrun.opentechf.org/docs/internals/rabbitmq); the
decision that made it a package is `docs/DECISIONS.md` D171. A task-shaped
introduction is the [guide](https://esrun.opentechf.org/docs/guides/rabbitmq).

## Connecting

`openConnection(url, settings?)` reads an `amqp://` or `amqps://` URL, opens a
socket, and completes the AMQP handshake. `amqps:` is TLS from the first byte.

```js
const connection = await openConnection("amqps://app:secret@rabbit.internal:5671/prod");
```

| URL part | |
| --- | --- |
| host, port | The broker. The port defaults to the scheme's own: 5672 for `amqp:`, 5671 for `amqps:`. |
| user, password | The SASL PLAIN credentials. Both default to `guest`. |
| path | The virtual host. One segment, so a `/` in a vhost is `%2F`. Defaults to `/`. |
| `amqps:` | TLS on connect. |
| query | `heartbeat`, `channel_max`, `frame_max`, `connection_timeout` (ms), `auth_mechanism` (`plain` only) and, on `amqps:`, `server_name_indication` and `verify=verify_peer` — the parameters RabbitMQ's own clients read. Any other parameter is refused with `ERR_RABBITMQ_UNSUPPORTED` rather than ignored; `cacertfile` names a file, so pass the PEM as `ca` instead. |

| Setting | |
| --- | --- |
| `username`, `password`, `vhost` | Override what the URL names. |
| `heartbeat` | Seconds to ask for. Defaults to 60. `0` means "no proposal" — the greater of the two sides' values is used, so a broker that wants heartbeats gets them. |
| `channelMax` | Channels to ask for. `0` (the default) accepts the broker's limit. |
| `frameMax` | Largest frame to accept. `0` (the default) accepts the broker's, never below the protocol's 4096. |
| `locale` | The message locale. RabbitMQ offers only `en_US`. |
| `clientProperties` | Extra client properties, merged over this client's own. |
| `connectTimeout` | Milliseconds the socket may take to open. Defaults to `timeout`; `0` waits as long as the operating system does. |
| `handshakeTimeout` | Milliseconds the opening handshake may take. Defaults to `timeout`. |
| `timeout` | Milliseconds any one method waits for its answer. Defaults to 30 000. A channel whose request goes unanswered is closed, since a late answer would otherwise settle the next request. |
| `ca`, `servername` | For `amqps:`: extra trust anchors as PEM (a private certificate authority), and the name the certificate must carry when it is not the URL's host. |

`parseAmqpUrl(url)` returns the same parts as a plain object, without dialling.

### The connection

| | |
| --- | --- |
| `connection.channel(options?)` | Opens a channel — the surface an application works in. |
| `connection.close()` | Closes the connection the way the protocol wants, then the socket. |
| `usable` | Whether it is still fit to use. |
| `failure` | The error that ended it, or `null`. |
| `server` | What the broker said about itself: `version`, `properties`, `mechanisms`, `locales`. |
| `channelMax`, `frameMax`, `heartbeat` | What the two sides agreed to. |
| `onBlocked`, `onUnblocked` | The broker asked a publisher to slow down, and lifted it. |
| `blocked`, `whenUnblocked()` | The broker's alarm reason while it blocks publishing (`null` otherwise), and a promise for the end of it. |
| `onClosed` | It ended, for any reason (`null` when closed on purpose). |
| `openChannel(handler)`, `closeChannel(n)`, `hasChannel(n)` | The low-level channel bookkeeping, for a caller driving its own handler. |

## Channels and topology

`connection.channel()` returns a `Channel`. A connection multiplexes many; each
is numbered from 1, and a channel lives until it is closed — by you, or by the
broker with a reason.

| | |
| --- | --- |
| `exchangeDeclare(name, type, options?)` | `type` is `direct`, `fanout`, `topic`, `headers`, or a broker plugin's. |
| `exchangeDelete(name, options?)` | |
| `exchangeBind(destination, source, routingKey?, options?)` | One exchange publishes into another. |
| `exchangeUnbind(destination, source, routingKey?, options?)` | |
| `queueDeclare(name?, options?)` | An empty name asks the broker to name it; the answer carries the name. |
| `queueDelete(queue, options?)` | Answers how many messages it held. |
| `queuePurge(queue)` | Answers how many it dropped. |
| `queueBind(queue, exchange, routingKey?, options?)` | |
| `queueUnbind(queue, exchange, routingKey?, options?)` | |

Declaration options are the protocol's booleans, each defaulting to `false`:
`passive`, `durable`, `autoDelete`, and on a queue `exclusive`; on an exchange
`internal` (fed by other exchanges, not publishers). Every declaration also
takes `arguments`, a `Map<string, FieldValue>` for broker extensions such as
`x-message-ttl` or a dead-letter policy. `queueDelete` adds `ifUnused` and
`ifEmpty`; `exchangeDelete` adds `ifUnused`. A declare answers a
`QueueDeclaration` — `queue`, `messageCount`, `consumerCount`; the others answer
nothing.

| Channel member | |
| --- | --- |
| `number` | Its number on the connection. |
| `usable`, `failure` | Whether it is open, and why it closed. |
| `close()` | Closes it and waits for the broker to agree. |
| `whenClosed()` | A promise for the end, with the reason (`null` when deliberate) — a promise rather than a hook so a subscriber can wait without taking `onClose`. |
| `onDelivery`, `onCancel`, `onReturn`, `onClose` | The raw hooks the consumer layer builds on. |
| `qos(options?)` | `prefetchCount` messages unacknowledged at once; `prefetchSize` (RabbitMQ accepts only 0); `global`: `false` (default) limits each consumer on the channel on its own, `true` the channel as a whole — RabbitMQ's meaning of the flag, not the specification's. |
| `get(queue, options?)` | Fetches one message, or `null` when the queue is empty. `noAck` settles it on delivery. |
| `recover(requeue?)` | Asks the broker to redeliver everything unacknowledged. |

## Publishing

`channel.publish(exchange, routingKey, body, options?)` publishes to an
exchange. The default exchange (`""`) routes by queue name. `body` is a string
or bytes.

```js
await channel.publish("", "jobs", JSON.stringify(job), {
  contentType: "application/json",
  deliveryMode: 2,          // persistent
  messageId: job.id,
});
```

| Property | |
| --- | --- |
| `contentType`, `contentEncoding` | MIME type and encoding. |
| `headers` | A `Map<string, FieldValue>` of application headers. |
| `deliveryMode` | `1` transient, `2` persistent. |
| `priority` | 0–9. |
| `correlationId`, `replyTo`, `messageId` | As the AMQP basic class names them. |
| `expiration` | Per-message TTL, in milliseconds, as a string. |
| `timestamp` | Seconds since the epoch, a `bigint`. |
| `type`, `userId`, `appId`, `clusterId` | |
| `mandatory` | Return the message — via `onReturn` — rather than drop it when nothing is bound to accept it. |
| `timeout` | Milliseconds the whole publish may take — waiting out a blocked connection, writing, and the confirm. Defaults to the channel's `timeout`; `0` waits forever. A timed-out publish rejects with `ERR_RABBITMQ_TIMEOUT`, and may still have reached the broker. |

While the broker blocks the connection (a memory or disk alarm; it stops reading
the socket), a publish waits at the client instead of queueing behind it, and
goes out when the broker unblocks — or fails at its `timeout`.

### Publisher confirms

`confirmSelect()` puts the channel in confirm mode. From then on the broker
acknowledges every publish, and `publish` does not resolve until it has — so
awaiting the publish awaits the message's fate. A publish the broker refuses (a
queue gone, a full queue it is configured to reject) rejects with
`ERR_RABBITMQ_NACK`, and one left unconfirmed past its `timeout` rejects with
`ERR_RABBITMQ_TIMEOUT`. `waitForConfirms(timeout?)` awaits every publish so far
at once, without holding one at a time.

## Consuming

`subscribe(channel, queue, options?)` starts a consumer and returns a
`Subscription`, an async iterable of deliveries. It holds at most `prefetch`
unacknowledged deliveries at once — 250 (`DEFAULT_PREFETCH`, within RabbitMQ's
recommended 100–300) unless the channel already has a prefetch from
`channel.qos()` or the subscription names one.

```js
const subscription = await subscribe(channel, "jobs", { prefetch: 16 });
for await (const delivery of subscription) {
  await handle(JSON.parse(new TextDecoder().decode(delivery.body)));
  await delivery.ack();
}
```

| Consume option | |
| --- | --- |
| `consumerTag` | A tag of your choosing; one (`esrun-<n>`) is generated otherwise. Unique per channel. |
| `noAck` | Handle the message as acknowledged the moment it is delivered. Off by default, which is what makes a crash lose nothing. On such a subscription `ack`/`nack`/`reject` refuse rather than send a frame the broker would treat as a protocol violation. |
| `prefetch` | Deliveries unacknowledged at once, for this consumer. Defaults to the channel's `qos()` prefetch, else 250. `0` asks for no limit. Ignored with `noAck`. |
| `exclusive` | Only this consumer may use the queue, and it is deleted when it ends. |
| `arguments` | Consumer arguments, such as `x-priority`. |

A `Delivery` is a `Message` — `consumerTag`, `deliveryTag`, `redelivered`,
`exchange`, `routingKey`, `properties`, `body` — plus `ack(options?)`,
`nack(options?)` and `reject(options?)`, which name the delivery's own tag.
`ack` takes `multiple`; `nack` takes `multiple` and `requeue`; `reject` takes
`requeue`. `requeue` defaults to `true`.

Around the loop: `subscription.consumerTag`, `subscription.queue`,
`subscription.closed`, `subscription.cancellation` (`{ byBroker, error }`) and
`await subscription.cancel()`. Breaking out of a `for await` cancels the
consumer, and cancelling hands deliveries that arrived but were not read back
to the queue (`basic.reject` with requeue) — the broker does not take them back
on a cancel, so dropping them would leave them unacknowledged until the channel
closes. A
broker-initiated cancellation ends the iterator with a `Cancellation` naming the
broker.

## Request/reply

`RpcClient` wraps a channel's direct reply-to, so a call is one `await`. Create
one per channel; it consumes the `amq.rabbitmq.reply-to` pseudo-queue with
`noAck` before it returns.

```js
import { openConnection, RpcClient } from "@opentf/esrun-rabbitmq";

const rpc = await RpcClient.open(await (await openConnection(url)).channel());
const answer = await rpc.call("fib", "10", { timeout: 5_000 });
console.log(new TextDecoder().decode(answer.body));
```

`call(routingKey, body, options?)` publishes a request carrying
`replyTo: amq.rabbitmq.reply-to` and a generated `esrun-rpc-<n>` correlation id
(or one you give), and resolves with the reply `Message`. A responder answers to
the `replyTo` on the request, echoing the `correlationId`. Options are the
publish properties plus `exchange` (default `""`), `timeout` (milliseconds,
default 30 000; `0` waits forever) and `mandatory`. Replies are matched by id,
so concurrent calls on one channel are told apart and a stray or unknown reply
is dropped. A call left unanswered past its timeout rejects with
`ERR_RABBITMQ_TIMEOUT`; a call in flight when the channel closes rejects with the
channel's reason, and a reply consumer the broker cancels closes the client the
same way. `rpc.pending` says how many calls are waiting and `rpc.closed` whether
it is done; `await rpc.close()` cancels the reply consumer and fails whatever is
still pending.

## Recovery

`openSession(url | urls, options?)` opens a `Session`: a connection that is
dialled again when it is lost, with channels and subscriptions that carry on
across it (DECISIONS D177). It resolves once the first connection is open.

```js
import { openSession } from "@opentf/esrun-rabbitmq";

const session = await openSession(["amqp://rabbit-1/", "amqp://rabbit-2/"]);
const jobs = await session.channel({
  confirm: true,
  setup: (channel) => channel.queueDeclare("jobs", { durable: true }),
});

await jobs.publish("", "jobs", "hello");
for await (const delivery of jobs.subscribe("jobs", { prefetch: 16 })) {
  await handle(delivery);
  await delivery.ack();
}
```

After a lost connection the session dials again — round-robin over the URLs,
with exponential backoff and jitter — then reopens each channel, re-selects
confirms, runs its `setup`, and re-consumes each subscription with its original
options. Nothing else is restored: topology is whatever `setup` declares, so
`setup` must be safe to run again, as a declaration is.

| Session option | |
| --- | --- |
| every connection setting | As for `openConnection`. `timeout` (30 000) also bounds how long the first connection is retried. |
| `reconnect` | `{ initialDelay, maxDelay }` in ms — the backoff bounds. Default 500 and 30 000. |
| `dial` | How to open the socket, as for the pool. |

| Session member | |
| --- | --- |
| `channel({ setup, confirm, timeout })` | A `RecoveringChannel`, open and set up when it resolves. |
| `connection`, `connected`, `whenConnected(timeout?)` | The live connection (`null` while reconnecting), and a promise for it. |
| `onConnect`, `onDisconnect`, `onError` | A connection opened; one was lost; a retry or a channel's setup failed and will be retried. |
| `close()` | Stops recovering and closes everything. |

A `RecoveringChannel` has `publish(exchange, routingKey, body, options?)`,
`subscribe(queue, options?)` (returning a `RecoveringSubscription`, an async
iterable of deliveries), `current` (the live `Channel`, or `null`),
`whenReady(timeout?)` and `close()`. `subscribe`'s `queue` may be a function,
read on every re-consume, for a queue `setup` declares afresh.

| | |
| --- | --- |
| What recovers | A lost socket, missed heartbeats, an unreadable frame, a broker-forced `connection.close` (a node restart's `320`). A channel the broker closes is reopened on the same connection. |
| What does not | The application's own `close()`. A consumer the broker cancels (its queue deleted) ends its subscription. |
| The first connection | Retried for transient failures until `timeout`; refused credentials, a version mismatch or an unsupported URL reject at once. |
| A publish while disconnected | Waits for the channel to return, within its `timeout`. |
| A publish unconfirmed at the drop | Rejects — its fate is unknown — and is **not** sent again. |
| Deliveries from before the drop | `ack`/`nack`/`reject` resolve without sending (their tags died with the channel), and ones buffered but not read are dropped; the broker redelivers both with `redelivered: true`. Processing is at-least-once. |

## The pool

`Pool` holds several connections to one broker, lends them one at a time, and
dials a fresh one when one dies.

```js
import { Pool } from "@opentf/esrun-rabbitmq";

const pool = new Pool("amqp://localhost/", { size: 4 });
await pool.withChannel((channel) => channel.publish("", "jobs", "hello"));
await pool.close();
```

| | |
| --- | --- |
| `size` | The most connections held open at once. Defaults to 4. |
| `onConnect` | Called with each connection once it is open and before it is lent out — the first one and every replacement. This is where an application re-declares topology and re-subscribes after a connection was lost; a failure here discards the connection and fails the borrow. |
| `dial` | How to open the socket. Defaults to `netTransport`; a test passes a scripted transport. |

A pool also takes every connection setting above. Borrow with
`acquire()`/`release(connection)` for a longer lease such as a subscription's,
or `withConnection(fn)` and `withChannel(fn)` for a scoped one — `withChannel`
opens and closes a channel around the function. `pool.size`, `pool.idle` and
`pool.busy` are the counts. `close()` closes every connection and refuses
further borrows with `ERR_RABBITMQ_CLOSED`.

Recovery is exactly the dial: an idle connection that has died is dropped when
it is next reached for, and a borrowed one that died is dropped on release, so
the next borrow dials a replacement. The pool does **not** replay topology or
re-subscribe — a broker cannot restore an exclusive or auto-delete resource, and
a silent replay would hide the gap rather than close it. For a long-lived
consumer or publisher that should carry on across a lost connection, use a
session (above).

## Errors

Every failure is a `RabbitError` (or its `ProtocolError` subclass) with the
broker's own `replyCode` and `replyText` kept verbatim, and a `code` to branch
on. `soft` says what is still usable: a channel exception kills the channel and
leaves the connection standing, a connection exception kills everything.

```js
import { RabbitError } from "@opentf/esrun-rabbitmq";

try {
  await channel.queueDeclare("jobs", { durable: true });
} catch (e) {
  if (e instanceof RabbitError && e.soft) await channel.queueDeclare("jobs");
  else throw e;
}
```

| Code | |
| --- | --- |
| `ERR_RABBITMQ_PROTOCOL` | The bytes were not AMQP — this client's or the broker's framing disagreeing. |
| `ERR_RABBITMQ_CONNECTION` | The socket could not be opened, was lost, or a frame could not be read. |
| `ERR_RABBITMQ_CLOSED` | The connection was closed, by this client or the broker. |
| `ERR_RABBITMQ_TIMEOUT` | Something did not answer within `timeout`. |
| `ERR_RABBITMQ_HANDSHAKE` | The handshake failed: a version or a tuning value that cannot be agreed. |
| `ERR_RABBITMQ_AUTH` | The broker refused the credentials or the virtual host. |
| `ERR_RABBITMQ_CHANNEL` | A channel was used in a way the protocol does not allow. |
| `ERR_RABBITMQ_SERVER` | The broker refused a method; `soft` says whether only its channel died. |
| `ERR_RABBITMQ_NACK` | The broker would not accept a published message, in publisher-confirm mode. |
| `ERR_RABBITMQ_UNSUPPORTED` | Something this client does not implement was asked for. |

## Testing locally

```sh
docker run -d --name rabbitmq -p 5672:5672 rabbitmq:4-alpine
```

Then `openConnection("amqp://guest:guest@127.0.0.1:5672/")` — or point
`test/run.sh` at a broker through `RABBITMQ_URL` and it runs the end-to-end
suite against it. `test/rabbitmq-server.sh` starts one and prints the variable.

## Not yet

AMQP 1.0 and the RabbitMQ Streams protocol, the management HTTP API, the `tx`
(transactions) class, the `access` class, and `basic.recover-async`.
