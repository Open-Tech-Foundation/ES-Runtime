// The consumer layer, against the scripted broker.
//
// What matters here is the routing by consumer tag, the buffering before a
// reader is waiting, the two ways a subscription ends — a cancel of ours, a
// cancel of the broker's — and that a broken channel ends it with the reason.
import { exit } from "runtime:process";
import { subscribe } from "../../dist/consumer.js";
import { is, ok, report } from "./assert.mjs";
import { openChannel, sendContent } from "./broker.mjs";

const enc = (text) => new TextEncoder().encode(text);
const dec = (bytes) => new TextDecoder().decode(bytes);
const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

/** The broker's side of a delivery: method, header, body. */
function deliver(broker, number, tag, consumerTag, body, properties = {}) {
  return sendContent(
    broker,
    "basic.deliver",
    { consumerTag, deliveryTag: tag, redelivered: false, exchange: "e", routingKey: "r" },
    number,
    properties,
    body,
  );
}

/** Subscribes and answers the broker's half, returning the live subscription. */
async function start(broker, channel, number, queue, options) {
  const subscribing = subscribe(channel, queue, options);
  const consume = await broker.expect("basic.consume");
  await broker.send("basic.consume-ok", { consumerTag: consume.args.consumerTag }, number);
  return { subscription: await subscribing, consume };
}

// -- subscribing and acknowledging -----------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const { subscription, consume } = await start(broker, channel, number, "jobs");

  is(consume.args.queue, "jobs", "the consumer names its queue");
  is(consume.args.noAck, false, "acknowledgement is manual by default");
  is(consume.args.noLocal, false, "no-local is off");
  ok(
    typeof consume.args.consumerTag === "string" && consume.args.consumerTag.startsWith("esrun-"),
    "the tag is chosen by us, before any delivery can arrive",
  );
  is(subscription.consumerTag, consume.args.consumerTag, "the subscription knows its tag");
  is(subscription.queue, "jobs", "and its queue");
  ok(subscription.closed === false, "it starts open");

  const iterator = subscription[Symbol.asyncIterator]();
  const next = iterator.next();
  await deliver(broker, number, 1n, subscription.consumerTag, enc("one"), { deliveryMode: 2 });
  const first = await next;
  ok(first.done === false, "a delivery arrives");
  is(dec(first.value.body), "one", "with its body");
  ok(first.value.deliveryTag === 1n, "and its delivery tag");
  is(first.value.consumerTag, subscription.consumerTag, "and the consumer it was for");
  is(first.value.properties.deliveryMode, 2, "and its properties");

  await first.value.ack();
  const ack = await broker.expect("basic.ack");
  ok(ack.args.deliveryTag === 1n, "the acknowledgement names the delivery");
  is(ack.args.multiple, false, "and acknowledges just that one");
}

// A delivery that arrives before anyone is reading is held, not lost.
{
  const { broker, channel, number } = await openChannel();
  const { subscription } = await start(broker, channel, number, "jobs");
  await deliver(broker, number, 1n, subscription.consumerTag, enc("first"));
  await deliver(broker, number, 2n, subscription.consumerTag, enc("second"));
  await settle();

  const bodies = [];
  const consuming = (async () => {
    for await (const delivery of subscription) {
      bodies.push(dec(delivery.body));
      await delivery.ack();
      if (bodies.length === 2) break;
    }
  })();

  const cancel = await broker.expect("basic.cancel");
  is(cancel.args.consumerTag, subscription.consumerTag, "leaving the loop cancels the consumer");
  await broker.send("basic.cancel-ok", { consumerTag: subscription.consumerTag }, number);
  await consuming;
  is(bodies.join(","), "first,second", "both deliveries are yielded, in order");
}

// -- cancellation -----------------------------------------------------------

// Breaking out of the loop cancels the consumer with the broker.
{
  const { broker, channel, number } = await openChannel();
  const { subscription } = await start(broker, channel, number, "jobs");

  const seen = [];
  const consuming = (async () => {
    for await (const delivery of subscription) {
      seen.push(dec(delivery.body));
      await delivery.ack();
      break;
    }
  })();

  await deliver(broker, number, 1n, subscription.consumerTag, enc("once"));
  await broker.expect("basic.ack");
  const cancel = await broker.expect("basic.cancel");
  is(cancel.args.consumerTag, subscription.consumerTag, "a break cancels the consumer");
  await broker.send("basic.cancel-ok", { consumerTag: subscription.consumerTag }, number);
  await consuming;

  is(seen.join(","), "once", "the one delivery was handled");
  ok(subscription.closed, "and the subscription is closed");
  is(subscription.cancellation.byBroker, false, "as a cancellation of ours");
}

// An explicit cancel does the same without a loop.
{
  const { broker, channel, number } = await openChannel();
  const { subscription } = await start(broker, channel, number, "jobs");
  const cancelling = subscription.cancel();
  const cancel = await broker.expect("basic.cancel");
  is(cancel.args.consumerTag, subscription.consumerTag, "cancel names the consumer");
  await broker.send("basic.cancel-ok", { consumerTag: subscription.consumerTag }, number);
  await cancelling;
  ok(subscription.closed, "the subscription is closed");
}

// The broker cancelling the consumer ends the iteration.
{
  const { broker, channel, number } = await openChannel();
  const { subscription } = await start(broker, channel, number, "jobs");
  const next = subscription[Symbol.asyncIterator]().next();
  await broker.send(
    "basic.cancel",
    { consumerTag: subscription.consumerTag, noWait: false },
    number,
  );
  await broker.expect("basic.cancel-ok");
  const result = await next;
  ok(result.done === true, "the iteration ends when the broker cancels");
  ok(subscription.closed, "and the subscription is closed");
  is(subscription.cancellation.byBroker, true, "as the broker's doing");
  is(subscription.cancellation.error, null, "with no error");
}

// A broken channel ends the subscription with the reason.
{
  const { broker, channel, number } = await openChannel();
  const { subscription } = await start(broker, channel, number, "jobs");
  const next = subscription[Symbol.asyncIterator]().next();
  const failure = next.then(
    () => null,
    (e) => e,
  );
  await broker.send(
    "channel.close",
    { replyCode: 500, replyText: "boom", classId: 0, methodId: 0 },
    number,
  );
  await broker.expect("channel.close-ok");
  const reason = await failure;
  is(reason?.replyCode, 500, "the pending read fails with the channel's reason");
  ok(subscription.closed, "and the subscription is closed");
}

// -- acknowledgements when there are none ----------------------------------

{
  const { broker, channel, number } = await openChannel();
  const { subscription, consume } = await start(broker, channel, number, "jobs", { noAck: true });
  is(consume.args.noAck, true, "noAck is passed to the broker");
  const iterator = subscription[Symbol.asyncIterator]();
  const next = iterator.next();
  await deliver(broker, number, 1n, subscription.consumerTag, enc("fire and forget"));
  const delivery = (await next).value;
  let refused = null;
  try {
    await delivery.ack();
  } catch (e) {
    refused = e;
  }
  is(
    refused?.code,
    "ERR_RABBITMQ_CHANNEL",
    "acknowledging a noAck subscription is refused rather than sent",
  );
}

// -- many subscriptions on one channel -------------------------------------

{
  const { broker, channel, number } = await openChannel();
  const first = await start(broker, channel, number, "one");
  const second = await start(broker, channel, number, "two");
  ok(first.subscription.consumerTag !== second.subscription.consumerTag, "the tags differ");

  const firstNext = first.subscription[Symbol.asyncIterator]().next();
  const secondNext = second.subscription[Symbol.asyncIterator]().next();
  await deliver(broker, number, 1n, second.subscription.consumerTag, enc("to two"));
  await deliver(broker, number, 2n, first.subscription.consumerTag, enc("to one"));

  is(dec((await firstNext).value.body), "to one", "a delivery is routed by its consumer tag");
  is(dec((await secondNext).value.body), "to two", "and the other to its own");
}

if (report("consumer") > 0) exit(1);
