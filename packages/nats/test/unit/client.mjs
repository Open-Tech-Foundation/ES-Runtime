// The client: publish, subscribe, and request/reply, against a scripted server.
import { exit } from "runtime:process";
import { decodeHeaders, NatsClient, NatsErrorCode } from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const TEXT = new TextDecoder();

async function client(options = {}) {
  const broker = fakeBroker();
  const pending = NatsClient.over(broker.transport, { reconnect: false, ...options });
  await broker.accept();
  const connected = await pending;
  return { client: connected, broker };
}

// -- publish and subscribe --------------------------------------------------

{
  const { client: c, broker } = await client();
  const subscription = c.subscribe("events.user");
  const sub = await broker.next();
  is(sub.kind, "sub", "a subscribe reaches the server");
  is(sub.subject, "events.user", "on the right subject");

  await c.publish("events.user", "hi");
  const pub = await broker.next();
  is(pub.kind, "pub", "a publish reaches the server");
  is(TEXT.decode(pub.payload), "hi", "with its payload");

  broker.sendMsg("events.user", subscription.sid, "reply");
  const message = await subscription.nextMessage(1000);
  is(message.text(), "reply", "and a routed message is delivered");
  ok(message instanceof Object, "as a message object");
  await c.close();
}

// -- routing is by subscription id ------------------------------------------

{
  const { client: c, broker } = await client();
  const first = c.subscribe("a.one");
  const second = c.subscribe("a.two");
  await broker.next();
  await broker.next();

  broker.sendMsg("a.two", second.sid, "two");
  broker.sendMsg("a.one", first.sid, "one");

  is((await second.nextMessage(1000)).text(), "two", "the second subscription gets its message");
  is((await first.nextMessage(1000)).text(), "one", "and the first gets its own");
  await c.close();
}

// -- queue groups -----------------------------------------------------------

{
  const { client: c, broker } = await client();
  c.subscribe("jobs", { queue: "workers" });
  const sub = await broker.next();
  is(sub.queue, "workers", "a queue group is carried on SUB");
  await c.close();
}

// -- unsubscribe ------------------------------------------------------------

{
  const { client: c, broker } = await client();
  const subscription = c.subscribe("a.b");
  await broker.next();
  subscription.unsubscribe();
  const unsub = await broker.next();
  is(unsub.kind, "unsub", "unsubscribe reaches the server");
  is(unsub.sid, subscription.sid, "for the right subscription");
  ok(subscription.closed, "and ends the iterable");
  await c.close();
}

// -- headers ----------------------------------------------------------------

{
  const { client: c, broker } = await client();
  await c.publish("a.b", "body", { headers: { "Nats-Msg-Id": "m1", "X-Two": ["a", "b"] } });
  const pub = await broker.next();
  ok(pub.headers !== null, "a publish with headers is an HPUB");
  const decoded = decodeHeaders(pub.headers);
  is(decoded.headers["Nats-Msg-Id"][0], "m1", "the header survives");
  is(decoded.headers["X-Two"].length, 2, "and a repeated header keeps its values");
  is(TEXT.decode(pub.payload), "body", "the payload still arrives");
  await c.close();
}

{
  const { client: c, broker } = await client();
  const subscription = c.subscribe("a.b");
  await broker.next();
  broker.sendHmsg("a.b", subscription.sid, "NATS/1.0\r\nK: v\r\n\r\n", "body");
  const message = await subscription.nextMessage(1000);
  is(message.header("k"), "v", "a header is readable case-insensitively");
  is(message.text(), "body", "with the payload");
  await c.close();
}

// -- request / reply --------------------------------------------------------

{
  const { client: c, broker } = await client();
  const pending = c.request("svc.echo", "ping", { timeout: 1000 });
  const sub = await broker.next();
  is(sub.kind, "sub", "a request subscribes an inbox first");
  const pub = await broker.next();
  is(pub.kind, "pub", "then publishes");
  is(pub.subject, "svc.echo", "to the service subject");
  is(pub.reply, sub.subject, "with the inbox as the reply subject");
  is(TEXT.decode(pub.payload), "ping", "and the request payload");

  broker.sendMsg(pub.reply, sub.sid, "pong");
  const reply = await pending;
  is(reply.text(), "pong", "the reply comes back");
  const dropped = await broker.next();
  is(dropped.kind, "unsub", "and the inbox is unsubscribed");
  await c.close();
}

// -- request with no responder ----------------------------------------------

{
  const { client: c, broker } = await client();
  const pending = c.request("svc.missing", "ping", { timeout: 1000 });
  const sub = await broker.next();
  const pub = await broker.next();
  broker.sendHmsg(pub.reply, sub.sid, "NATS/1.0 503\r\n\r\n", "");
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  is(error?.code, NatsErrorCode.NoResponders, "a 503 status becomes a no-responder error");
  await c.close();
}

// -- timeouts ---------------------------------------------------------------

{
  const { client: c, broker } = await client();
  const subscription = c.subscribe("a.b");
  await broker.next();
  let error = null;
  try {
    await subscription.nextMessage(30);
  } catch (e) {
    error = e;
  }
  is(error?.code, NatsErrorCode.Timeout, "an unanswered read times out");
  await c.close();
}

// -- flush ------------------------------------------------------------------

{
  const { client: c, broker } = await client();
  const flushing = c.flush(1000);
  const ping = await broker.next();
  is(ping.kind, "ping", "flush sends a PING");
  broker.sendPong();
  await flushing;
  ok(true, "and resolves");
  await c.close();
}

// -- close ends the iterables -----------------------------------------------

{
  const { client: c } = await client();
  const subscription = c.subscribe("a.b");
  const reading = (async () => {
    for await (const _ of subscription) return "message";
    return "done";
  })();
  await c.close();
  is(await reading, "done", "closing the client ends a subscription iterator");
  ok(c.closed, "and the client is closed");
}

// -- a closed client refuses work -------------------------------------------

{
  const { client: c } = await client();
  await c.close();
  await throws(() => c.publish("a.b", "x"), "a closed client refuses a publish");
  await throws(() => c.flush(), "a closed client refuses a flush");
}

exit(report("client") > 0 ? 1 : 0);
