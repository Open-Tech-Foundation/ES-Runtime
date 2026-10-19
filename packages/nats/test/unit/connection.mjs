// The connection: the handshake, the command pump, keep-alive, and the reconnect
// that re-establishes subscriptions, all against a scripted server.
import { exit } from "runtime:process";
import { Connection, NatsErrorCode, openWithDial } from "../../dist/index.js";
import { is, ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const TEXT = new TextDecoder();
const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));

function deferred() {
  let resolve;
  const promise = new Promise((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

/** Connects over a fake broker, running the server's half concurrently. */
async function connect(settings = {}, broker = fakeBroker(), info = {}) {
  const pending = Connection.over(broker.transport, { reconnect: false, ...settings });
  const options = await broker.accept(info);
  const connection = await pending;
  return { connection, broker, options };
}

// -- the handshake ----------------------------------------------------------

{
  const { connection, options, broker } = await connect(
    { username: "u", password: "p", name: "n1" },
    fakeBroker(),
    { headers: true, max_payload: 2048 },
  );
  is(options.lang, "esrun", "CONNECT reports the client language");
  is(options.protocol, 1, "and protocol 1");
  is(options.user, "u", "and the user");
  is(options.pass, "p", "and the password");
  is(options.name, "n1", "and the connection name");
  is(options.headers, true, "and header support when the server offers it");
  is(options.no_responders, true, "and no-responder support alongside headers");
  is(connection.serverVersion, "2.10.0", "the server version is recorded");
  is(connection.maxPayload, 2048, "the payload ceiling is taken from INFO");
  ok(connection.connected, "the connection is up");
  await connection.close();
  ok(connection.closed, "close ends it");
  void broker;
}

{
  const { options } = await connect({ token: "secret" }, fakeBroker(), { headers: false });
  is(options.auth_token, "secret", "a token is sent as auth_token");
  is(options.headers, false, "headers are not claimed when the server has none");
  is(options.no_responders, false, "nor no-responders");
}

// -- TLS, upgraded after INFO -----------------------------------------------

{
  const broker = fakeBroker();
  const { options, connection } = await connect({ tls: true }, broker, { tls_required: true });
  is(broker.tlsUpgrades, 1, "a TLS request upgrades the transport once INFO arrives");
  is(options.tls_required, true, "and CONNECT says TLS is required");
  await connection.close();
}

{
  const broker = fakeBroker();
  const pending = Connection.over(broker.transport, { reconnect: false });
  broker.sendInfo({ tls_required: true });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  is(error?.code, NatsErrorCode.Handshake, "a server that requires TLS refuses a clear connection");
  is(broker.tlsUpgrades, 0, "and no upgrade is attempted");
}

// -- publish ----------------------------------------------------------------

{
  const { connection, broker } = await connect();
  await connection.publish("events.user", "hi");
  const command = await broker.next();
  is(command.kind, "pub", "the server receives a publish");
  is(command.subject, "events.user", "on the right subject");
  is(TEXT.decode(command.payload), "hi", "with the payload");
  is(command.reply, null, "and no reply subject");
  await connection.close();
}

{
  const { connection, broker } = await connect();
  await connection.publish("events.user", "hi", { reply: "_INBOX.x" });
  const command = await broker.next();
  is(command.reply, "_INBOX.x", "a reply subject is carried");
  await connection.close();
}

// -- subscribe and route ----------------------------------------------------

{
  const { connection, broker } = await connect({}, fakeBroker());
  const sid = connection.subscribe("a.b");
  const command = await broker.next();
  is(command.kind, "sub", "the server receives a subscribe");
  is(command.subject, "a.b", "for the right subject");
  is(command.sid, sid, "under the id the connection allocated");
  await connection.close();
}

{
  const received = [];
  const broker = fakeBroker();
  const pending = Connection.over(
    broker.transport,
    { reconnect: false },
    {
      onMessage: (message) => received.push(message),
    },
  );
  await broker.accept();
  const connection = await pending;

  const sid = connection.subscribe("a.b");
  await broker.next();
  broker.sendMsg("a.b", sid, "hello");
  await tick();
  is(received.length, 1, "a message routed to the subscription arrives");
  is(received[0].subject, "a.b", "with its subject");
  is(TEXT.decode(received[0].payload), "hello", "and its payload");
  await connection.close();
}

// -- flush ------------------------------------------------------------------

{
  const { connection, broker } = await connect();
  const flushing = connection.flush(1000);
  const command = await broker.next();
  is(command.kind, "ping", "flush sends a PING");
  broker.sendPong();
  await flushing;
  ok(true, "and resolves on the PONG");
  await connection.close();
}

// -- a server PING is answered ----------------------------------------------

{
  const { connection, broker } = await connect();
  broker.sendPing();
  const command = await broker.next();
  is(command.kind, "pong", "a server PING is answered with a PONG");
  await connection.close();
}

// -- a server -ERR fails the connection -------------------------------------

{
  const errors = [];
  const broker = fakeBroker();
  const pending = Connection.over(
    broker.transport,
    { reconnect: false },
    {
      onClosed: (error) => errors.push(error),
    },
  );
  await broker.accept();
  const connection = await pending;

  broker.sendErr("Authorization Violation");
  await tick();
  ok(connection.failure !== null, "an -ERR fails the connection");
  is(connection.failure.code, NatsErrorCode.Auth, "an authorization error is classified");
  is(errors.length, 1, "and the close handler is called once");
  await connection.close();
}

// -- payload ceiling --------------------------------------------------------

{
  const { connection } = await connect({}, fakeBroker(), { max_payload: 4 });
  let code = null;
  try {
    await connection.publish("a.b", "12345");
  } catch (e) {
    code = e.code;
  }
  is(code, NatsErrorCode.Payload, "a message over the server's maximum is refused");
  await connection.publish("a.b", "1234");
  ok(true, "and one at the limit is allowed");
  await connection.close();
}

// -- reconnect --------------------------------------------------------------

{
  const broker1 = fakeBroker();
  const broker2 = fakeBroker();
  let dials = 0;
  const dial = async () => {
    const broker = dials++ === 0 ? broker1 : broker2;
    void broker.accept();
    return broker.transport;
  };

  const disconnected = deferred();
  const reconnected = deferred();
  const connection = await openWithDial(
    "nats://h:4222",
    { reconnect: { maxAttempts: 5, wait: 1 } },
    {
      onDisconnect: () => disconnected.resolve(),
      onReconnect: () => reconnected.resolve(),
    },
    dial,
  );

  const sid = connection.subscribe("a.b");
  const firstSub = await broker1.next();
  is(firstSub.kind, "sub", "the subscription reaches the first server");

  broker1.close();
  await disconnected.promise;
  ok(!connection.connected, "the connection reports itself down");

  const publishing = connection.publish("a.b", "after");
  await reconnected.promise;
  ok(connection.connected, "and comes back");

  const resub = await broker2.next();
  is(resub.kind, "sub", "the subscription is re-established");
  is(resub.subject, "a.b", "on the same subject");
  is(resub.sid, sid, "under the same id");

  const published = await broker2.next();
  is(published.kind, "pub", "the publish issued while down goes out after recovery");
  is(TEXT.decode(published.payload), "after", "with its payload");
  await publishing;
  await connection.close();
}

// -- a deliberate close does not reconnect ----------------------------------

{
  const { connection, broker } = await connect();
  await connection.close();
  is(await broker.next(100), null, "closing the connection closes the pipe");
  ok(connection.closed, "and reports closed");
}

// -- a timed-out flush keeps its place in the PONG queue ---------------------

{
  const { connection, broker } = await connect();
  let timedOut = null;
  const first = connection.flush(20).catch((e) => {
    timedOut = e;
  });
  const firstPing = await broker.next();
  is(firstPing.kind, "ping", "the first flush sends a PING");
  await first;
  is(timedOut?.code, NatsErrorCode.Timeout, "and times out when no PONG comes");

  const second = connection.flush(1000);
  const secondPing = await broker.next();
  is(secondPing.kind, "ping", "a second flush sends its own PING");

  broker.sendPong(); // the PONG owed to the first, timed-out flush
  const early = await Promise.race([second.then(() => "settled"), tick(30).then(() => "pending")]);
  is(early, "pending", "a late PONG for the timed-out flush does not settle the next one");

  broker.sendPong(); // the PONG for the second flush
  await second;
  ok(true, "which settles on its own PONG");
  await connection.close();
}

// -- a publish parked on a reconnect is released when the connection closes ---

{
  const broker = fakeBroker();
  let dials = 0;
  const dial = async () => {
    if (dials++ === 0) {
      void broker.accept();
      return broker.transport;
    }
    throw new Error("the server is gone");
  };
  const disconnected = deferred();
  const connection = await openWithDial(
    "nats://h:4222",
    { reconnect: { maxAttempts: 100, wait: 5 } },
    { onDisconnect: () => disconnected.resolve() },
    dial,
  );

  broker.close();
  await disconnected.promise;
  ok(!connection.connected, "the connection drops");

  let error = null;
  const publishing = connection.publish("a.b", "held").catch((e) => {
    error = e;
  });
  await tick(30); // the reconnect loop fails a few times; the connection stays down

  await connection.close();
  await publishing;
  is(
    error?.code,
    NatsErrorCode.Closed,
    "a publish waiting on the reconnect is released by close, not left hanging",
  );
}

exit(report("connection") > 0 ? 1 : 0);
