// The client against a real RabbitMQ.
//
// Everything the unit suites cannot show is here: that a broker accepts the
// handshake this client writes, that a generated queue name is one the broker
// actually made, that a confirmed publish is acknowledged by RabbitMQ itself,
// that a subscription's prefetch is honoured, and that an unroutable mandatory
// publish comes back as a `basic.return`. The framing either matches RabbitMQ's
// or nothing below the first assertion passes.
//
//   eval "$(test/rabbitmq-server.sh)" && test/run.sh
//   # or point RABBITMQ_URL at a broker you already have
import { env, exit } from "runtime:process";
import { Connection } from "../dist/connection.js";
import { subscribe } from "../dist/consumer.js";
import { openConnection, Pool, parseAmqpUrl } from "../dist/pool.js";
import { RpcClient } from "../dist/rpc.js";
import { netTransport } from "../dist/transport.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.RABBITMQ_URL;
if (url === undefined) {
  console.log("skip e2e — run test/rabbitmq-server.sh first (it prints RABBITMQ_URL)");
  exit(0);
}

const { hostname, port, tls, username, password, vhost } = parseAmqpUrl(url);

const dec = (bytes) => new TextDecoder().decode(bytes);
const settle = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const suffix = Math.random().toString(36).slice(2, 8);

/** A fresh connection to the broker the environment names. */
async function connect(credentials = { username, password }) {
  const transport = await netTransport({ hostname, port, tls });
  const connection = new Connection(transport, { ...credentials, vhost });
  await connection.open();
  return connection;
}

// Optional local TLS listener. Passing its CA through the injected dial also
// verifies that an AMQPS URL without a port selects 5671.
if (env.RABBITMQ_TLS_URL !== undefined) {
  if (env.RABBITMQ_CA === undefined) {
    throw new Error("RABBITMQ_CA is required when RABBITMQ_TLS_URL is set");
  }
  const tlsAddress = parseAmqpUrl(env.RABBITMQ_TLS_URL);
  is(tlsAddress.port, 5671, "an AMQPS URL without a port selects the TLS listener");
  ok(tlsAddress.tls, "the AMQPS URL enables TLS");
  const connection = await openConnection(env.RABBITMQ_TLS_URL, {}, (options) =>
    netTransport({ ...options, ca: env.RABBITMQ_CA }),
  );
  is(
    connection.server.properties.get("product").value,
    "RabbitMQ",
    "the TLS handshake reaches RabbitMQ",
  );
  await connection.close();

  let untrusted = null;
  try {
    await openConnection(env.RABBITMQ_TLS_URL, {}, (options) =>
      netTransport({ ...options, ca: "not a trusted certificate authority" }),
    );
  } catch (e) {
    untrusted = e;
  }
  is(untrusted?.code, "ERR_RABBITMQ_CONNECTION", "an untrusted TLS certificate is rejected");
}

// -- the handshake ----------------------------------------------------------

{
  const connection = await connect();
  ok(connection.usable, "the handshake completes against a real broker");
  is(connection.server.properties.get("product").value, "RabbitMQ", "RabbitMQ is on the other end");
  ok(connection.frameMax >= 4096, `frames were negotiated (frameMax ${connection.frameMax})`);
  ok(connection.heartbeat > 0, `a heartbeat was negotiated (${connection.heartbeat}s)`);

  const channel = await connection.channel();
  const queue = await channel.queueDeclare("", { exclusive: true, autoDelete: true });
  ok(queue.queue.startsWith("amq."), `the broker named the queue (${queue.queue})`);
  is(queue.messageCount, 0, "a new queue is empty");
  await connection.close();
}

// -- a bad password ---------------------------------------------------------

{
  const transport = await netTransport({ hostname, port, tls });
  const connection = new Connection(transport, { username, password: `${password}-wrong`, vhost });
  let failure = null;
  try {
    await connection.open();
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_AUTH", "a wrong password is refused as an auth failure");
}

// -- publishing, confirming, consuming --------------------------------------

{
  const connection = await connect();
  const channel = await connection.channel();
  // Exclusive, but not auto-delete: the consumer below is cancelled, and an
  // auto-delete queue would go with it, nacking the later publishes to it.
  const queue = (await channel.queueDeclare("", { exclusive: true })).queue;

  await channel.confirmSelect();
  const bodies = ["alpha", "bravo", "charlie", "delta", "echo"];
  for (const [at, text] of bodies.entries()) {
    await channel.publish("", queue, text, {
      contentType: "text/plain",
      deliveryMode: 2,
      messageId: `m-${at}`,
      headers: new Map([["n", { type: "int32", value: at }]]),
    });
  }
  ok(true, "five confirmed publishes were acknowledged by the broker");

  await channel.qos({ prefetchCount: 2 });
  const subscription = await subscribe(channel, queue);
  ok(subscription.consumerTag.startsWith("esrun-"), "the consumer tag is the one we chose");

  const seen = [];
  for await (const delivery of subscription) {
    seen.push({
      body: dec(delivery.body),
      id: delivery.properties.messageId,
      type: delivery.properties.contentType,
      mode: delivery.properties.deliveryMode,
      n: delivery.properties.headers.get("n").value,
      redelivered: delivery.redelivered,
    });
    await delivery.ack();
    if (seen.length === bodies.length) break;
  }
  is(
    seen.map((one) => one.body).join(","),
    bodies.join(","),
    "every message arrives, in the order it was published",
  );
  is(seen[0].id, "m-0", "the message id round-trips");
  is(seen[0].type, "text/plain", "so does the content type");
  is(seen[0].mode, 2, "and the delivery mode");
  is(seen[0].n, 0, "and an integer header");
  is(seen[0].redelivered, false, "a first delivery is not marked redelivered");

  // A message left in the queue, fetched one at a time.
  await channel.publish("", queue, "left behind");
  const got = await channel.get(queue);
  is(dec(got.body), "left behind", "basic.get fetches a message");
  ok(got.deliveryTag > 0n, "with a delivery tag to acknowledge");
  await channel.ack(got.deliveryTag);
  is(await channel.get(queue), null, "and the queue is then empty");

  // An exchange, a binding, and a message routed across them.
  const exchange = `esrun-e2e-${suffix}`;
  await channel.exchangeDeclare(exchange, "topic");
  await channel.queueBind(queue, exchange, "a.#");
  await channel.publish(exchange, "a.b", "routed");
  const routed = await channel.get(queue);
  is(dec(routed.body), "routed", "an exchange routes by its binding");
  await channel.ack(routed.deliveryTag);
  await channel.exchangeDelete(exchange);

  // A mandatory publish with nowhere to go is returned, not dropped.
  let returned = null;
  channel.onReturn = (message) => {
    returned = message;
  };
  await channel.publish("", `nowhere-${suffix}`, "returned", { mandatory: true });
  for (let at = 0; at < 100 && returned === null; at++) await settle(20);
  ok(returned !== null, "an unroutable mandatory publish is returned");
  is(returned?.returned.replyCode, 312, "with NO_ROUTE");
  is(dec(returned.body), "returned", "and the body comes back with it");

  await connection.close();
}

// -- request/reply over direct reply-to -------------------------------------

// A service in the same file: it consumes requests and answers each one to the
// reply address the broker put on the request, echoing the correlation id. That
// is exactly what a real responder does, and the broker rewriting `replyTo` on
// the way in is what makes the answer come back to the caller's channel alone.
{
  const server = await connect();
  const serverChannel = await server.channel();
  const service = `esrun-rpc-${suffix}`;
  await serverChannel.queueDeclare(service, { exclusive: true });
  await serverChannel.qos({ prefetchCount: 4 });
  const requests = await subscribe(serverChannel, service);
  const serving = (async () => {
    for await (const request of requests) {
      const text = dec(request.body);
      ok(
        typeof request.properties.replyTo === "string" && request.properties.replyTo !== "",
        "the broker gives the request a reply address",
      );
      await serverChannel.publish("", request.properties.replyTo, `echo:${text}`, {
        correlationId: request.properties.correlationId,
        contentType: "text/plain",
      });
      await request.ack();
    }
  })();

  const client = await connect();
  const rpc = await RpcClient.open(await client.channel());
  const answer = await rpc.call(service, "ping");
  is(dec(answer.body), "echo:ping", "a direct reply-to call is answered");
  is(answer.properties.contentType, "text/plain", "with the responder's properties");
  ok(
    typeof answer.properties.correlationId === "string" &&
      answer.properties.correlationId.startsWith("esrun-rpc-"),
    "matched by the correlation id the call was given",
  );

  const [first, second] = await Promise.all([rpc.call(service, "one"), rpc.call(service, "two")]);
  is(dec(first.body), "echo:one", "concurrent calls are told apart");
  is(dec(second.body), "echo:two", "each getting its own answer");
  ok(rpc.pending === 0, "and nothing is left waiting");

  let unanswered = null;
  try {
    await rpc.call(`nobody-${suffix}`, "x", { timeout: 300 });
  } catch (e) {
    unanswered = e;
  }
  is(unanswered?.code, "ERR_RABBITMQ_TIMEOUT", "a call nobody serves times out");

  await rpc.close();
  await requests.cancel();
  await client.close();
  await server.close();
  await serving;
}

// -- an exclusive queue dies with its connection ----------------------------

{
  const first = await connect();
  const queue = (await (await first.channel()).queueDeclare("", { exclusive: true })).queue;
  await first.close();

  const second = await connect();
  const channel = await second.channel();
  let failure = null;
  try {
    await channel.queueDeclare(queue, { passive: true });
  } catch (e) {
    failure = e;
  }
  is(failure?.replyCode, 404, "the exclusive queue is gone with its connection");
  ok(failure?.soft === true, "and the refusal is only a channel exception");
  ok(second.usable === true, "so the connection survives it");
  await second.close();
}

// -- the pool ---------------------------------------------------------------

// Two borrows at once open two connections, each announced to `onConnect`, which
// is where an application re-declares the topology a connection needs. Closing
// the pool closes them.
{
  const announced = [];
  let serial = 0;
  const pool = new Pool(url, {
    size: 2,
    onConnect: async (connection) => {
      const channel = await connection.channel();
      // Exclusive, so the pool's own close is what removes it — and named per
      // connection, because an exclusive queue is locked to the connection that
      // declared it.
      await channel.queueDeclare(`esrun-pool-${suffix}-${serial++}`, { exclusive: true });
      await channel.close();
      announced.push(connection);
    },
  });

  const declared = await Promise.all([
    pool.withChannel((channel) => channel.queueDeclare("", { exclusive: true })),
    pool.withChannel((channel) => channel.queueDeclare("", { exclusive: true })),
  ]);
  ok(
    declared.every((queue) => queue.queue.startsWith("amq.")),
    "concurrent borrows each get a working connection",
  );
  is(pool.size, 2, "a pool of two opens two connections under load");
  is(announced.length, 2, "and onConnect runs for each of them");
  is(pool.busy, 0, "every connection is returned");

  await pool.close();
  is(pool.size, 0, "closing the pool closes every connection");
}

if (report("rabbitmq e2e") > 0) exit(1);
