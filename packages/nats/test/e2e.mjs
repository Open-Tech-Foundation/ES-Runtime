// End-to-end: a real NATS, over a real socket, with nothing scripted.
//
// Skips itself when NATS_URL is not set, so `test/run.sh` stays runnable
// without Docker. Set it with `eval "$(test/nats-server.sh)"`.
import { env, exit } from "runtime:process";
import { NatsClient, NatsErrorCode } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.NATS_URL ?? "";
if (url === "") {
  console.log('skip e2e (set NATS_URL, e.g. eval "$(test/nats-server.sh)")');
  exit(0);
}

const suffix = Math.random().toString(36).slice(2, 10);
const subject = (name) => `esrun.e2e.${suffix}.${name}`;
const settings = { reconnect: false, timeout: 5000 };

async function collect(subscription, count, timeout = 2000) {
  const out = [];
  const deadline = Date.now() + timeout;
  while (out.length < count) {
    const left = deadline - Date.now();
    if (left <= 0) break;
    try {
      out.push(await subscription.nextMessage(left));
    } catch {
      break;
    }
  }
  return out;
}

// -- connect and flush ------------------------------------------------------

const client = await NatsClient.connect(url, settings);
ok(client.connected, "the client connects to a real server");
ok(typeof client.serverVersion === "string", "and learns the server's version");
ok(client.maxPayload > 0, "and its payload ceiling");
await client.flush(5000);
ok(true, "and a flush is answered with a PONG");

// -- publish and subscribe --------------------------------------------------

{
  const subscription = client.subscribe(subject("basic"));
  await client.flush(5000);
  await client.publish(subject("basic"), "hello");
  const message = await subscription.nextMessage(5000);
  is(message.text(), "hello", "a published message comes back");
  is(message.subject, subject("basic"), "on its subject");
  ok(message.reply === null, "with no reply subject");
}

// -- wildcards --------------------------------------------------------------

{
  const subscription = client.subscribe(subject("wild.>"));
  await client.flush(5000);
  await client.publish(subject("wild.a.b"), "deep");
  await client.publish(subject("other"), "ignored");
  const message = await subscription.nextMessage(5000);
  is(message.text(), "deep", "a `>` subscription sees a nested subject");
  const nothing = await collect(subscription, 1, 300);
  is(nothing.length, 0, "and does not see a subject outside it");
  await subscription.unsubscribe();
}

// -- queue groups -----------------------------------------------------------

{
  const other = await NatsClient.connect(url, settings);
  const a = client.subscribe(subject("jobs"), { queue: "workers" });
  const b = other.subscribe(subject("jobs"), { queue: "workers" });
  await client.flush(5000);
  await other.flush(5000);
  for (let i = 0; i < 4; i++) await client.publish(subject("jobs"), `job-${i}`);
  const got = [...(await collect(a, 4, 3000)), ...(await collect(b, 4, 3000))];
  is(got.length, 4, "a queue group delivers every message exactly once across members");
  await a.unsubscribe();
  await b.unsubscribe();
  await other.close();
}

// -- request and reply ------------------------------------------------------

{
  const service = client.subscribe(subject("echo"));
  void (async () => {
    for await (const message of service) {
      await client.publish(message.reply, `echo:${message.text()}`);
    }
  })();
  await client.flush(5000);
  const reply = await client.request(subject("echo"), "hi", { timeout: 5000 });
  is(reply.text(), "echo:hi", "a request gets the service's reply");
  await service.unsubscribe();
}

// -- no responders ----------------------------------------------------------

{
  let error = null;
  try {
    await client.request(subject("nobody"), "hi", { timeout: 5000 });
  } catch (e) {
    error = e;
  }
  is(error?.code, NatsErrorCode.NoResponders, "a request with no service reports no responders");
}

// -- headers ----------------------------------------------------------------

{
  const subscription = client.subscribe(subject("headers"));
  await client.flush(5000);
  await client.publish(subject("headers"), "body", { headers: { "Nats-Msg-Id": "abc" } });
  const message = await subscription.nextMessage(5000);
  is(message.header("nats-msg-id"), "abc", "a header survives a round trip");
  await subscription.unsubscribe();
}

// -- unsubscribe stops delivery ---------------------------------------------

{
  const subscription = client.subscribe(subject("gone"));
  await client.flush(5000);
  await subscription.unsubscribe();
  await client.flush(5000);
  await client.publish(subject("gone"), "lost");
  const nothing = await collect(subscription, 1, 300);
  is(nothing.length, 0, "an unsubscribed subject delivers nothing");
}

await client.close();
ok(client.closed, "the client closes");

// -- TLS --------------------------------------------------------------------

{
  const tlsUrl = env.NATS_TLS_URL ?? "";
  if (tlsUrl !== "") {
    const tlsClient = await NatsClient.connect(tlsUrl, {
      ...settings,
      tls: true,
      ca: env.NATS_CA ?? null,
      servername: "localhost",
    });
    const subscription = tlsClient.subscribe(subject("tls"));
    await tlsClient.flush(5000);
    await tlsClient.publish(subject("tls"), "secure");
    is((await subscription.nextMessage(5000)).text(), "secure", "a TLS connection works");
    await tlsClient.close();

    let error = null;
    try {
      await NatsClient.connect(tlsUrl, { ...settings, tls: true, servername: "localhost" });
    } catch (e) {
      error = e;
    }
    is(
      error?.code,
      NatsErrorCode.Connection,
      "and a certificate the client cannot verify is refused",
    );
  }
}

exit(report("e2e") > 0 ? 1 : 0);
