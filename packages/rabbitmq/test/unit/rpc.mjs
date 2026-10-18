// The request/reply layer, against the scripted broker.
//
// What matters here is that the reply pseudo-queue is consumed the one way it
// allows (noAck), that a request carries `replyTo` and a correlation id, that
// concurrent calls are told apart by that id rather than by order, and that a
// call which can never be answered — timed out, closed, or on a dead channel —
// fails rather than hangs.
import { exit } from "runtime:process";
import { FrameType } from "../../dist/protocol/frame.js";
import { decodeContentHeader } from "../../dist/protocol/properties.js";
import { DIRECT_REPLY_TO, RpcClient } from "../../dist/rpc.js";
import { is, ok, report } from "./assert.mjs";
import { openChannel, sendContent } from "./broker.mjs";

const enc = (text) => new TextEncoder().encode(text);
const dec = (bytes) => new TextDecoder().decode(bytes);

/** Opens a client and answers the broker's half of the reply consumer. */
async function open(channel, broker, number, options) {
  const opening = RpcClient.open(channel, options);
  const consume = await broker.expect("basic.consume");
  await broker.send("basic.consume-ok", { consumerTag: consume.args.consumerTag }, number);
  return { rpc: await opening, consume };
}

/** Reads the method, header, and body of one publish the client sent. */
async function readPublish(broker, number) {
  const publish = await broker.expect("basic.publish");
  const header = await broker.expectFrame(FrameType.header, number);
  const body = await broker.expectFrame(FrameType.body, number);
  return {
    publish,
    properties: decodeContentHeader(header.payload).properties,
    body: body.payload,
  };
}

/** Sends a reply to the reply consumer: method, header, body. */
function reply(broker, number, tag, consumerTag, correlationId, body) {
  return sendContent(
    broker,
    "basic.deliver",
    { consumerTag, deliveryTag: tag, redelivered: false, exchange: "", routingKey: "" },
    number,
    { correlationId, contentType: "text/plain" },
    body,
  );
}

async function rejection(promise) {
  try {
    await promise;
    return null;
  } catch (e) {
    return e;
  }
}

// -- opening and one call ---------------------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const { rpc, consume } = await open(channel, broker, number);

  is(DIRECT_REPLY_TO, "amq.rabbitmq.reply-to", "the pseudo-queue is the one RabbitMQ names");
  is(consume.args.queue, DIRECT_REPLY_TO, "the client consumes the reply pseudo-queue");
  is(consume.args.noAck, true, "and only in the noAck way it allows");
  ok(
    typeof consume.args.consumerTag === "string" && consume.args.consumerTag.startsWith("esrun-"),
    "with a tag it chose",
  );

  const calling = rpc.call("fib", "10");
  const sent = await readPublish(broker, number);
  is(sent.publish.args.exchange, "", "a call defaults to the default exchange");
  is(sent.publish.args.routingKey, "fib", "and routes to what was asked for");
  is(sent.properties.replyTo, DIRECT_REPLY_TO, "the request says where the reply goes");
  ok(
    typeof sent.properties.correlationId === "string" && sent.properties.correlationId !== "",
    "and carries a correlation id",
  );
  ok(rpc.pending === 1, "the call is waiting while it is in flight");
  is(dec(sent.body), "10", "the request body is the body");

  await reply(
    broker,
    number,
    1n,
    consume.args.consumerTag,
    sent.properties.correlationId,
    enc("55"),
  );
  const answer = await calling;
  is(dec(answer.body), "55", "the answer is the reply's body");
  is(answer.properties.contentType, "text/plain", "with the reply's properties");
  ok(rpc.pending === 0, "and nothing is left waiting");
  const closing = rpc.close();
  const cancel = await broker.expect("basic.cancel");
  is(cancel.args.consumerTag, consume.args.consumerTag, "closing cancels the reply consumer");
  await broker.send("basic.cancel-ok", { consumerTag: consume.args.consumerTag }, number);
  await closing;
  ok(rpc.closed, "closing cancels the reply consumer");
}

// -- concurrent calls, told apart by correlation id -------------------------

{
  const { broker, channel, number } = await openChannel();
  const { rpc, consume } = await open(channel, broker, number);

  const first = rpc.call("work", "one");
  const firstSent = await readPublish(broker, number);
  const second = rpc.call("work", "two");
  const secondSent = await readPublish(broker, number);
  ok(
    firstSent.properties.correlationId !== secondSent.properties.correlationId,
    "each call gets its own correlation id",
  );
  ok(rpc.pending === 2, "both are in flight at once");

  // Answered out of order: the id is what routes them, not the arrival order.
  await reply(
    broker,
    number,
    2n,
    consume.args.consumerTag,
    secondSent.properties.correlationId,
    enc("two!"),
  );
  await reply(
    broker,
    number,
    1n,
    consume.args.consumerTag,
    firstSent.properties.correlationId,
    enc("one!"),
  );
  is(dec((await first).body), "one!", "the first call gets its own answer");
  is(dec((await second).body), "two!", "and the second gets its own");

  // A reply nobody is waiting for — a duplicate, or a stranger — is dropped.
  await reply(broker, number, 3n, consume.args.consumerTag, "not-a-call", enc("stray"));
  const late = rpc.call("work", "late", { correlationId: "mine" });
  const lateSent = await readPublish(broker, number);
  is(lateSent.properties.correlationId, "mine", "a chosen correlation id is used as given");
  await reply(broker, number, 4n, consume.args.consumerTag, "mine", enc("made it"));
  is(dec((await late).body), "made it", "and the call still resolves past the stray reply");
}

// -- ending without an answer ----------------------------------------------

// A call waits out its timeout when no reply ever comes.
{
  const { broker, channel, number } = await openChannel();
  const { rpc } = await open(channel, broker, number, { timeout: 40 });
  const calling = rpc.call("nowhere", "x");
  const failure = rejection(calling);
  await readPublish(broker, number);
  is((await failure)?.code, "ERR_RABBITMQ_TIMEOUT", "a call with no reply times out");
  ok(rpc.pending === 0, "the timed-out call is dropped from the pending set");
}

// A dead channel fails every call still waiting, with the channel's reason.
{
  const { broker, channel, number } = await openChannel();
  const { rpc } = await open(channel, broker, number);
  const calling = rpc.call("work", "x");
  const failure = rejection(calling);
  await readPublish(broker, number);
  await broker.send(
    "channel.close",
    { replyCode: 500, replyText: "boom", classId: 0, methodId: 0 },
    number,
  );
  await broker.expect("channel.close-ok");
  is((await failure)?.replyCode, 500, "a pending call fails with the channel's reason");
  ok(rpc.closed, "and the client is closed with it");
}

// Closing fails what is still waiting and refuses what comes after.
{
  const { broker, channel, number } = await openChannel();
  const { rpc, consume } = await open(channel, broker, number);
  const calling = rpc.call("work", "x");
  const failure = rejection(calling);
  await readPublish(broker, number);

  const closing = rpc.close();
  const cancel = await broker.expect("basic.cancel");
  is(cancel.args.consumerTag, consume.args.consumerTag, "closing cancels the reply consumer");
  await broker.send("basic.cancel-ok", { consumerTag: consume.args.consumerTag }, number);
  await closing;

  is((await failure)?.code, "ERR_RABBITMQ_CLOSED", "a call in flight when it closes is refused");
  ok(rpc.closed, "the client is closed");
  const after = await rejection(rpc.call("work", "x"));
  is(after?.code, "ERR_RABBITMQ_CLOSED", "and a later call is refused too");
}

if (report("rpc") > 0) exit(1);
