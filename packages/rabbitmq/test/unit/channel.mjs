// The channel, against the same scripted broker the connection tests use.
//
// What is worth pinning here is the pairing of a method with its answer, the
// assembly of a method, header and body frames into one message, and the
// publisher-confirm bookkeeping — the arithmetic that a live broker will not
// produce on demand and that a reader cannot check by eye.
import { exit } from "runtime:process";
import { decodeContentHeader, encodeContentHeader } from "../../dist/protocol/properties.js";
import { is, ok, report } from "./assert.mjs";
import { openChannel, sendContent } from "./broker.mjs";

const enc = (text) => new TextEncoder().encode(text);
const dec = (bytes) => new TextDecoder().decode(bytes);

/** The broker's side of a publish: the method, its header, and its single body frame. */
async function takePublish(broker, channel) {
  const method = await broker.expect("basic.publish");
  const header = await broker.expectFrame(2, channel);
  const body = await broker.expectFrame(3, channel);
  return { method, header, body };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

// -- declaring topology -----------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  ok(channel.number === number, "the channel knows its number");

  // A queue with no name: the broker names it, and the answer carries it.
  const declared = channel.queueDeclare("", { durable: true });
  const declare = await broker.expect("queue.declare");
  is(declare.args.queue, "", "an empty name asks the broker to choose");
  is(declare.args.durable, true, "durable is what was asked for");
  is(declare.args.passive, false, "passive defaults off");
  is(declare.args.exclusive, false, "exclusive defaults off");
  is(declare.args.autoDelete, false, "auto-delete defaults off");
  await broker.send(
    "queue.declare-ok",
    { queue: "amq.gen-1", messageCount: 3, consumerCount: 1 },
    number,
  );
  const result = await declared;
  is(result.queue, "amq.gen-1", "the broker's generated name comes back");
  is(result.messageCount, 3, "with the message count");
  is(result.consumerCount, 1, "and the consumer count");

  const exchange = channel.exchangeDeclare("logs", "topic", { durable: true });
  const declared2 = await broker.expect("exchange.declare");
  is(declared2.args.exchange, "logs", "the exchange name");
  is(declared2.args.type, "topic", "and its type");
  is(declared2.args.internal, false, "internal defaults off");
  await broker.send("exchange.declare-ok", {}, number);
  await exchange;

  const bound = channel.queueBind("q", "logs", "a.#", { arguments: new Map() });
  const bind = await broker.expect("queue.bind");
  is(bind.args.queue, "q", "a binding names its queue");
  is(bind.args.exchange, "logs", "and its exchange");
  is(bind.args.routingKey, "a.#", "and its routing key");
  await broker.send("queue.bind-ok", {}, number);
  await bound;

  const unbound = channel.queueUnbind("q", "logs", "a.#");
  await broker.expect("queue.unbind");
  await broker.send("queue.unbind-ok", {}, number);
  await unbound;

  const purged = channel.queuePurge("q");
  await broker.expect("queue.purge");
  await broker.send("queue.purge-ok", { messageCount: 7 }, number);
  is(await purged, 7, "a purge answers with what it dropped");

  const deleted = channel.queueDelete("q", { ifEmpty: true });
  const del = await broker.expect("queue.delete");
  is(del.args.ifEmpty, true, "delete passes ifEmpty");
  is(del.args.ifUnused, false, "and defaults ifUnused off");
  await broker.send("queue.delete-ok", { messageCount: 2 }, number);
  is(await deleted, 2, "a delete answers with the queue's depth");
}

// -- publishing -------------------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const body = enc("hello, world");
  const published = channel.publish("amq.topic", "a.b", body, {
    deliveryMode: 2,
    contentType: "text/plain",
    correlationId: "c-1",
  });
  const { method, header, body: frame } = await takePublish(broker, number);
  is(method.args.exchange, "amq.topic", "the publish names its exchange");
  is(method.args.routingKey, "a.b", "and its routing key");
  is(method.args.mandatory, false, "mandatory defaults off");
  const decoded = decodeContentHeader(header.payload);
  ok(decoded.bodySize === BigInt(body.length), "the header says how much body follows");
  is(decoded.properties.deliveryMode, 2, "the delivery mode is carried");
  is(decoded.properties.contentType, "text/plain", "so is the content type");
  is(decoded.properties.correlationId, "c-1", "and the correlation id");
  ok(frame.payload.length === body.length, "the body arrives whole");
  is(dec(frame.payload), "hello, world", "and unchanged");
  await published;
}

// A body larger than one frame is split, not refused.
{
  const { broker, channel, number } = await openChannel({
    channelMax: 0,
    frameMax: 4096,
    heartbeat: 0,
  });
  const body = new Uint8Array(5000).fill(65);
  const published = channel.publish("e", "r", body);
  await broker.expect("basic.publish");
  await broker.expectFrame(2, number);
  const first = await broker.expectFrame(3, number);
  const second = await broker.expectFrame(3, number);
  is(first.payload.length, 4088, "a body frame is frame-max minus its overhead");
  is(second.payload.length, 5000 - 4088, "the remainder follows in a second frame");
  await published;
}

// -- qos and acknowledgements ----------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const qos = channel.qos({ prefetchCount: 10 });
  const qosMethod = await broker.expect("basic.qos");
  is(qosMethod.args.prefetchCount, 10, "the prefetch count is passed");
  is(qosMethod.args.prefetchSize, 0, "prefetch size defaults to 0");
  is(qosMethod.args.global, false, "and global defaults off");
  await broker.send("basic.qos-ok", {}, number);
  await qos;

  await channel.ack(5n);
  const ack = await broker.expect("basic.ack");
  ok(ack.args.deliveryTag === 5n, "a delivery tag rides as a bigint");
  is(ack.args.multiple, false, "a single ack defaults multiple off");

  await channel.ack(6, { multiple: true });
  const ack2 = await broker.expect("basic.ack");
  is(ack2.args.multiple, true, "multiple is passed through");

  await channel.nack(5n, { requeue: false });
  const nack = await broker.expect("basic.nack");
  ok(nack.args.deliveryTag === 5n, "a nack names its tag");
  is(nack.args.requeue, false, "and its requeue");

  await channel.reject(7n);
  const reject = await broker.expect("basic.reject");
  ok(reject.args.deliveryTag === 7n, "a reject names its tag");
  is(reject.args.requeue, true, "and requeues by default");

  const recovered = channel.recover();
  await broker.expect("basic.recover");
  await broker.send("basic.recover-ok", {}, number);
  await recovered;
}

// -- getting ----------------------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const got = channel.get("q");
  const get = await broker.expect("basic.get");
  is(get.args.queue, "q", "a get names its queue");
  is(get.args.noAck, false, "and acknowledges by default");
  await sendContent(
    broker,
    "basic.get-ok",
    { deliveryTag: 5n, redelivered: true, exchange: "e", routingKey: "r", messageCount: 2 },
    number,
    { headers: new Map([["k", { type: "string", value: "v" }]]), deliveryMode: 2 },
    enc("payload"),
  );
  const message = await got;
  is(dec(message.body), "payload", "the body is assembled from its frame");
  is(message.routingKey, "r", "the envelope is kept");
  is(message.messageCount, 2, "so is the remaining count");
  is(message.redelivered, true, "and the redelivered flag");
  ok(message.deliveryTag === 5n, "with the delivery tag");
  is(message.properties.deliveryMode, 2, "and the properties");
  is(message.properties.headers.get("k").value, "v", "including the headers");

  const empty = channel.get("q");
  await broker.expect("basic.get");
  await broker.send("basic.get-empty", {}, number);
  is(await empty, null, "an empty queue answers with null");
}

// -- publisher confirms -----------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const selecting = channel.confirmSelect();
  await broker.expect("confirm.select");
  await broker.send("confirm.select-ok", {}, number);
  await selecting;

  const published = channel.publish("e", "r", "one");
  await takePublish(broker, number);
  await broker.send("basic.ack", { deliveryTag: 1n, multiple: false }, number);
  await published;
  ok(true, "a confirmed publish resolves");

  const nacked = channel.publish("e", "r", "two");
  await takePublish(broker, number);
  await broker.send("basic.nack", { deliveryTag: 2n, multiple: false, requeue: false }, number);
  let failure = null;
  try {
    await nacked;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_NACK", "a nacked publish rejects");

  const first = channel.publish("e", "r", "three");
  const second = channel.publish("e", "r", "four");
  const all = channel.waitForConfirms();
  await takePublish(broker, number);
  await takePublish(broker, number);
  await broker.send("basic.ack", { deliveryTag: 4n, multiple: true }, number);
  await first;
  await second;
  await all;
  ok(true, "a multiple ack settles every publish it covers");
}

// -- a returned message -----------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  let returned = null;
  channel.onReturn = (message) => {
    returned = message;
  };
  const published = channel.publish("", "nowhere", "x", { mandatory: true });
  const { method } = await takePublish(broker, number);
  is(method.args.mandatory, true, "mandatory is passed through");
  await published;
  await sendContent(
    broker,
    "basic.return",
    { replyCode: 312, replyText: "NO_ROUTE", exchange: "", routingKey: "nowhere" },
    number,
    {},
    enc("x"),
  );
  await settle();
  ok(returned !== null, "the return reaches onReturn");
  is(returned.returned.replyCode, 312, "with the broker's reply code");
  is(returned.returned.replyText, "NO_ROUTE", "and its text");
  is(returned.routingKey, "nowhere", "and the routing key it tried");
}

// -- the broker closing the channel ----------------------------------------

{
  const { broker, channel, number } = await openChannel();
  let closed = null;
  channel.onClose = (error) => {
    closed = error;
  };

  const doomed = channel.queueDeclare("q");
  const failure = doomed.then(
    () => null,
    (e) => e,
  );
  await broker.expect("queue.declare");
  await broker.send(
    "channel.close",
    { replyCode: 406, replyText: "PRECONDITION_FAILED", classId: 50, methodId: 10 },
    number,
  );
  await broker.expect("channel.close-ok");

  is((await failure)?.replyCode, 406, "the outstanding request is rejected with the broker's code");
  ok(closed?.soft === true, "and the handler is told, softly");
  ok(channel.usable === false, "the channel is no longer usable");

  let refused = null;
  try {
    await channel.queueDeclare("q");
  } catch (e) {
    refused = e;
  }
  is(refused?.replyCode, 406, "and every later call fails with the same reason");
}

// -- closing ----------------------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  let closed;
  channel.onClose = (error) => {
    closed = error;
  };
  const closing = channel.close();
  const close = await broker.expect("channel.close");
  is(close.args.replyCode, 200, "a deliberate close says 200");
  await broker.send("channel.close-ok", {}, number);
  await closing;
  ok(channel.usable === false, "the channel is closed");
  ok(closed === null, "and a deliberate close reports no error");
}

// -- malformed content ------------------------------------------------------

// A body that overruns the header's declared size leaves the stream at an
// unknown boundary, so it is fatal — and the channel is told why.
{
  const { broker, channel, number } = await openChannel();
  let closed = null;
  channel.onClose = (error) => {
    closed = error;
  };
  await broker.send(
    "basic.deliver",
    { consumerTag: "c", deliveryTag: 1n, redelivered: false, exchange: "e", routingKey: "r" },
    number,
  );
  await broker.sendHeader(
    encodeContentHeader({ classId: 60, weight: 0, bodySize: 4n, properties: {} }),
    number,
  );
  await broker.sendBody([1, 2, 3, 4, 5, 6, 7, 8], number);
  await settle();
  is(closed?.code, "ERR_RABBITMQ_PROTOCOL", "an overrunning body is a protocol failure");
  ok(channel.usable === false, "and the channel is closed");
}

// A header with no method to announce it is a protocol failure too.
{
  const { broker, channel, number } = await openChannel();
  let closed = null;
  channel.onClose = (error) => {
    closed = error;
  };
  await broker.sendHeader(
    encodeContentHeader({ classId: 60, weight: 0, bodySize: 0n, properties: {} }),
    number,
  );
  await settle();
  is(closed?.code, "ERR_RABBITMQ_PROTOCOL", "a header without a method is a protocol failure");
}

// -- confirm numbering starts at confirm.select ------------------------------

// The broker counts a publish sent after `confirm.select` and before its
// `select-ok` as number 1. Starting the count at the answer misnumbered it and
// every confirm after it.
{
  const { broker, channel, number } = await openChannel();
  const selecting = channel.confirmSelect();
  await broker.expect("confirm.select");
  let published = false;
  const publishing = channel.publish("", "q", "early").then(() => {
    published = true;
  });
  await takePublish(broker, number);
  await broker.send("confirm.select-ok", {}, number);
  await selecting;
  await broker.send("basic.ack", { deliveryTag: 1n, multiple: false }, number);
  await publishing;
  ok(published, "a publish racing select-ok is confirmed by the broker's tag 1");
  const second = channel.publish("", "q", "next");
  await takePublish(broker, number);
  await broker.send("basic.ack", { deliveryTag: 2n, multiple: false }, number);
  await second;
  ok(true, "and the next publish is tag 2");
}

// -- a timed-out request closes the channel ---------------------------------

// Answers pair with requests by order; once one is missing, a late answer
// would settle the next request of its kind. So the channel is given up.
{
  const { broker, connection } = await openChannel();
  const opening = connection.channel({ timeout: 40 });
  const open = await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, open.channel);
  const channel = await opening;
  let failure = null;
  await channel.queueDeclare("a").catch((e) => {
    failure = e;
  });
  is(failure?.code, "ERR_RABBITMQ_TIMEOUT", "the unanswered declare times out");
  const close = await broker.expect("channel.close");
  is(close.channel, open.channel, "and the channel is closed");
  ok(!channel.usable, "it is no longer usable");
  is(channel.failure?.code, "ERR_RABBITMQ_TIMEOUT", "with the timeout as its reason");
  let refused = null;
  await channel.queueDeclare("b").catch((e) => {
    refused = e;
  });
  is(refused?.code, "ERR_RABBITMQ_TIMEOUT", "a later call is refused with that reason");
  await broker.send("channel.close-ok", {}, open.channel);
  await settle();
  is(channel.failure?.code, "ERR_RABBITMQ_TIMEOUT", "close-ok does not replace the reason");
  ok(connection.usable, "the connection itself is untouched");
}

// -- a method inside a message is a framing error -----------------------------

{
  const { broker, connection, number } = await openChannel();
  await broker.send(
    "basic.deliver",
    { consumerTag: "c", deliveryTag: 1n, redelivered: false, exchange: "", routingKey: "q" },
    number,
  );
  await broker.send(
    "basic.deliver",
    { consumerTag: "c", deliveryTag: 2n, redelivered: false, exchange: "", routingKey: "q" },
    number,
  );
  await settle();
  is(
    connection.failure?.code,
    "ERR_RABBITMQ_PROTOCOL",
    "a second method before the first message's header fails the connection",
  );
}

if (report("channel") > 0) exit(1);
