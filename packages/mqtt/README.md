# @opentf/esrun-mqtt

An MQTT client for [esrun](https://esrun.opentechf.org), speaking MQTT 5.0 and
falling back to 3.1.1, written entirely in JavaScript over `runtime:net`. There
is no native code in this package, and none was added to the runtime for it.

```sh
npm install @opentf/esrun-mqtt
```

```js
import { MqttClient } from "@opentf/esrun-mqtt";

const client = await MqttClient.connect("mqtt://test.mosquitto.org:1883", {
  clientId: "esrun-demo",
});

const subscription = await client.subscribe("sensors/+/temperature", { qos: 1 });
for await (const message of subscription) {
  console.log(message.topic, message.text());
}

await client.publish("sensors/kitchen/temperature", "21.5", { qos: 1 });
await client.close();
```

Run it with the network grant scoped to the broker:

```sh
esrun --allow-net=test.mosquitto.org:1883 app.js
```

The package's exports are the whole public surface; the wire protocol and its
codecs stay internal — except the packet codec, which is exported so a test or a
stub broker can use it. What it is made of, and why each rule is there, is on the
[internals page](https://esrun.opentechf.org/docs/internals/mqtt); the decision
that made it a package is `docs/DECISIONS.md` D172. A task-shaped introduction is
the [guide](https://esrun.opentechf.org/docs/guides/mqtt).

## Connecting

`MqttClient.connect(url, options?)` parses the URL, opens a socket and completes
the handshake, then starts the packet pump and keep-alive. `mqtts:` is TLS from
the first byte.

```js
const client = await MqttClient.connect("mqtts://app:secret@broker.internal:8883", {
  clientId: "worker-1",
  keepAlive: 30,
});
```

| URL part | |
| --- | --- |
| host, port | The broker. The port defaults to the scheme's own: 1883 for `mqtt:` (`tcp:`), 8883 for `mqtts:` (`ssl:`, `tls:`). |
| user, password | The credentials. Optional; percent-encoded places are decoded. |
| `mqtts:` | TLS on connect — MQTT has no in-band upgrade. |

| Setting | |
| --- | --- |
| `clientId` | The identifier the broker keys a session on. Empty or omitted asks the broker to assign one (5.0). |
| `username`, `password` | The credentials; override what the URL carries. `password` may be bytes. |
| `keepAlive` | Seconds to ask for. Defaults to 60; `0` disables it. A broker's `serverKeepAlive` overrides it (5.0). |
| `cleanStart` | Whether to start a fresh session. Defaults to `true`. |
| `will` | The last will: `{ topic, payload, qos, retain, properties? }`, published by the broker if this client vanishes. |
| `sessionExpiryInterval` | Seconds the session survives a disconnect (5.0). |
| `properties` | Extra CONNECT properties, merged over the ones derived from the fields above. |
| `protocolVersion` | `5` or `4` to pin a level. Omitted means negotiate: try 5.0, fall back to 3.1.1 when the broker will not speak it. |
| `timeout` | Milliseconds the handshake, and the default for a publish/subscribe, may take. Defaults to 30 000. |
| `ca`, `servername` | Trust anchors, as PEM, for a private authority; and the name the certificate must carry when it is not the host. |

`parseMqttUrl(url)` returns the same host and credentials as a plain object,
without dialling: `hostname`, `port`, `tls`, and optionally `username`/`password`.

| Client member | |
| --- | --- |
| `protocolLevel` | `5` for MQTT 5.0, `4` for 3.1.1 — what was negotiated. |
| `sessionPresent` | Whether the broker resumed a session for this client id. |
| `closed`, `failure` | Whether it is over, and the error that ended it (`null` while it stands). |
| `publish(topic, payload, options?)` | See below. |
| `subscribe(filter, options?)` | See below. |
| `unsubscribe(filters, timeout?)` | One filter or a list. |
| `request(topic, payload, options?)` | One `await` for a reply (5.0). |
| `ping(timeout?)` | A PINGREQ, resolved by its PINGRESP. Concurrent calls share the one exchange; a timeout fails the connection, since a PINGRESP carries no identifier to match it to. |
| `close(options?)` | A DISCONNECT, then the socket. `options` carries `reasonCode` and `properties`. |
| `over(transport, options?, level?)` | Connects over an already-open transport — the seam a test stands on. |

## Publishing

`publish(topic, payload, options?)` sends a message. `payload` is a string or
bytes. At QoS 0 it resolves once the bytes are written; at QoS 1 and 2 it
resolves only once the broker has acknowledged, so awaiting the publish awaits
the message's fate. A message the broker refuses rejects with `ERR_MQTT_PUBLISH`
carrying its reason code.

```js
await client.publish("events/user", JSON.stringify(user), {
  qos: 2,
  retain: true,
  contentType: "application/json",
  userProperties: [["source", "signup"]],
});
```

| Publish option | |
| --- | --- |
| `qos` | `0`, `1` or `2`. Defaults to `0`. |
| `retain` | Ask the broker to keep the message for future subscribers. Defaults to `false`. |
| `messageExpiryInterval` | Seconds before the broker drops it (5.0). |
| `contentType` | A MIME type (5.0). |
| `responseTopic`, `correlationData` | The reply plumbing `request` sets for you (5.0). |
| `userProperties` | An array of `[name, value]` pairs, in order (5.0). |
| `properties` | Any further 5.0 property, merged over the ones above. |
| `timeout` | Milliseconds to wait for the acknowledgement. Defaults to the client's `timeout`. |

## Subscribing

`subscribe(filter, options?)` registers a topic filter and returns a
`Subscription`, an async iterable of `Message`. The filter's wildcards — `+` for
one level, `#` for the rest — are matched here, and a filter may begin with
`$share/<group>/` to join a shared subscription.

```js
const subscription = await client.subscribe("jobs/#", { qos: 1 });

for await (const message of subscription) {
  await handle(message.json());
}
```

| Subscribe option | |
| --- | --- |
| `qos` | The maximum QoS to receive. Defaults to `0`. The broker grants at or below it. |
| `noLocal` | Do not send this client its own publishes back (5.0). |
| `retainAsPublished` | Keep the retained flag as published rather than clearing it (5.0). |
| `retainHandling` | `0` send retained now, `1` send if the subscription is new, `2` never (5.0). |
| `subscriptionIdentifier` | An identifier the broker echoes on matching messages (5.0). |
| `properties` | Any further 5.0 property. |
| `timeout` | Milliseconds to wait for the SUBACK. Defaults to the client's `timeout`. |

| Subscription member | |
| --- | --- |
| `filter` | The filter it was made with. |
| `grantedQos` | The QoS the broker granted. |
| `closed` | Whether it has ended. |
| `nextMessage(timeout?)` | The next message, rejecting with `ERR_MQTT_TIMEOUT` if none arrives. |
| `unsubscribe()` | Unsubscribes at the broker and ends the iterable. |
| `matches(topic)` | Whether a topic matches this filter. |

A `Message` is `topic`, `payload` (`Uint8Array`), `qos`, `retain`, `dup`, and the
5.0 `properties` (`{}` on a 3.1.1 connection), plus `text(encoding?)`, `json()`,
and the `correlationData`, `responseTopic` and `userProperties` shortcuts.

## Request/reply

MQTT 5.0 defines the convention and nothing more: publish with a response topic
and correlation data, and read the reply that carries the same data back.
`request` sets up the reply subscription, publishes, and resolves the first reply
whose correlation data matches. It needs 5.0 and refuses on 3.1.1 rather than
downgrading to a fire-and-forget publish.

```js
const reply = await client.request("service/fib", "10", { timeout: 5_000 });
console.log(reply.text());
```

The responder is an ordinary subscriber: it reads `message.responseTopic` and
`message.correlationData` off the request, and publishes its answer to the former
carrying the latter. An unanswered request rejects with `ERR_MQTT_TIMEOUT`.

## The lower layers

The `Connection` and the transport are exported for a caller that wants to drive
the protocol itself, or stand in for the broker. `openConnection(url, settings?,
handlers?)` opens one and reports packets through `ConnectionHandlers`
(`onMessage`, `onAck`, `onSuback`, `onUnsuback`, `onPingresp`, `onDisconnect`,
`onClosed`). `Connection.over(transport, settings?, handlers?, level?)` takes an
already-open `Transport` instead of dialling; `netTransport(options)` is the one
over `runtime:net`. A stand-in transport implements `read`, `write` and `close`;
`close` is called with `{ abort: true }` when the connection is being torn down
after a failure, so the implementation can drop a write still pending rather than
wait on it. The packet codec (`encodePacket`, `decodePacket`,
`PacketType`), the reason names (`reasonName`, `isErrorReason`) and the topic
rules (`topicMatches`, `validateTopicName`, `validateTopicFilter`) are exported
alongside them.

## Errors

Every failure is an `MqttError` (or its `ProtocolError` subclass) with a `code`,
the broker's own `reasonCode` kept verbatim, and its `reasonName` as the
specification spells it (`Not authorized`).

```js
import { MqttError, MqttErrorCode } from "@opentf/esrun-mqtt";

try {
  await client.publish("events/user", payload, { qos: 1 });
} catch (e) {
  if (e instanceof MqttError && e.code === MqttErrorCode.Publish) retry(payload);
  else throw e;
}
```

| Code | |
| --- | --- |
| `ERR_MQTT_PROTOCOL` | The bytes were not MQTT — this client's or the broker's framing disagreeing. |
| `ERR_MQTT_CONNECTION` | The socket could not be opened, was lost, or a packet could not be read. |
| `ERR_MQTT_CLOSED` | The connection was closed, by this client or the broker. |
| `ERR_MQTT_TIMEOUT` | Something did not answer within the time allowed. |
| `ERR_MQTT_HANDSHAKE` | The handshake failed, before or instead of a CONNACK. |
| `ERR_MQTT_AUTH` | The broker refused the credentials or the client identifier. |
| `ERR_MQTT_REFUSED` | The broker refused the CONNECT itself. |
| `ERR_MQTT_SUBSCRIPTION` | The broker refused one topic filter. |
| `ERR_MQTT_PUBLISH` | The broker refused one published message. |
| `ERR_MQTT_SERVER` | The broker sent a DISCONNECT carrying an error, or violated the protocol. |
| `ERR_MQTT_UNSUPPORTED` | Something this client does not implement was asked for. |

A lost socket or bytes that are not MQTT are fatal and latched: the stream is at
an unknown boundary, so the connection does not try to resynchronise. Reconnecting
is the application's decision — this client does not redial on its own.

## Testing locally

Mosquitto 2 needs a config before it will listen off loopback for an anonymous
client, so the harness writes one and starts the broker for you:

```sh
eval "$(test/mosquitto-server.sh)"   # starts eclipse-mosquitto:2, prints MQTT_URL
```

Then `MqttClient.connect("mqtt://127.0.0.1:1883")` — or point `test/run.sh` at a
broker of your own through `MQTT_URL` and it runs the end-to-end suite against it.

## Not yet

MQTT over WebSockets, MQTT 3.1 (the older wire format below 3.1.1), automatic
reconnection, and broker-side persistence management. Messages on a persistent
session are redelivered by the broker; deciding to redial and resubscribe is the
application's.
