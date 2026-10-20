// The pool, against scripted connections.
//
// The pool's whole job is bookkeeping — who is lent, who is idle, when a slot is
// free — and one policy: a connection that has died is thrown away and the next
// borrow dials a fresh one, with no topology replayed on its behalf. A scripted
// dial is what makes that testable: each dial is a fresh loopback the test can
// kill on cue, and no port is ever opened.
import { exit } from "runtime:process";
import { openConnection, Pool, parseAmqpUrl } from "../../dist/pool.js";
import { is, ok, report } from "./assert.mjs";
import { Broker, handshake, loopback } from "./broker.mjs";

const URL = "amqp://esrun:secret@broker.test:5673/prod";

const settle = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Waits for a condition, so a test does not guess how long a dial takes. */
async function until(predicate, what = "condition", timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`waited too long for ${what}`);
    await settle(1);
  }
}

/** A `dial` that scripts a fresh connection each time, and records the addresses. */
function dialer() {
  const brokers = [];
  const addresses = [];
  const dial = async (address) => {
    addresses.push(address);
    const pair = loopback();
    const broker = new Broker(pair.server);
    brokers.push(broker);
    void handshake(broker).catch(() => {});
    return pair.client;
  };
  return { dial, brokers, addresses };
}

/** Closes a pool, answering each still-live connection's `connection.close`. */
async function closePool(pool, brokers) {
  const closing = pool.close();
  await Promise.all(
    brokers.map(async (broker) => {
      await broker.expect("connection.close");
      await broker.send("connection.close-ok", {}, 0);
    }),
  );
  await closing;
}

async function rejection(promise) {
  try {
    await promise;
    return null;
  } catch (e) {
    return e;
  }
}

// -- reading the address ----------------------------------------------------

{
  const address = parseAmqpUrl("amqp://esrun:secret@host.example:5673/prod");
  is(address.hostname, "host.example", "the host comes from the URL");
  is(address.port, 5673, "so does the port");
  is(address.tls, false, "and amqp: is not TLS");
  is(address.username, "esrun", "the username");
  is(address.password, "secret", "the password");
  is(address.vhost, "prod", "and the vhost is the path");

  const plain = parseAmqpUrl("amqp://broker");
  is(plain.port, 5672, "amqp: without a port gets the plain AMQP port");

  const bare = parseAmqpUrl("amqps://broker");
  is(bare.port, 5671, "amqps: without a port gets the AMQP-over-TLS port");
  ok(bare.tls, "amqps: is TLS");
  is(bare.username, "guest", "an absent username defaults to RabbitMQ's");
  is(bare.password, "guest", "and so does the password");
  is(bare.vhost, "/", "and the vhost defaults to /");

  const encoded = parseAmqpUrl("amqp://u%40x:p%2Fw@h/%2F");
  is(encoded.username, "u@x", "a percent-encoded username is decoded");
  is(encoded.password, "p/w", "so is the password");
  is(encoded.vhost, "/", "and a %2F vhost is the root vhost, not a path");

  const wrong = await rejection(Promise.resolve().then(() => parseAmqpUrl("redis://h")));
  is(wrong?.code, "ERR_RABBITMQ_CONNECTION", "a URL that is not AMQP is refused");
}

// -- one connection ---------------------------------------------------------

{
  const { dial, brokers, addresses } = dialer();
  const connection = await openConnection(URL, { heartbeat: 0 }, dial);
  ok(connection.usable, "openConnection completes the handshake");
  is(addresses[0].hostname, "broker.test", "and dials the address the URL named");
  is(addresses[0].port, 5673, "on the port it named");
  is(addresses[0].tls, false, "with the transport the scheme asked for");

  const closing = connection.close();
  await brokers[0].expect("connection.close");
  await brokers[0].send("connection.close-ok", {}, 0);
  await closing;
}

// -- reuse and counters -----------------------------------------------------

{
  const { dial, brokers } = dialer();
  const pool = new Pool(URL, { heartbeat: 0, dial, size: 2 });

  let first;
  let second;
  await pool.withConnection(async (connection) => {
    first = connection;
    is(pool.busy, 1, "a borrowed connection is busy");
    is(pool.idle, 0, "and not idle");
  });
  is(pool.idle, 1, "releasing an idle connection puts it back");
  is(pool.busy, 0, "and clears busy");

  await pool.withConnection(async (connection) => {
    second = connection;
  });
  ok(first === second, "the next borrow reuses it");
  is(brokers.length, 1, "rather than dialling another");
  is(pool.size, 1, "the pool holds one connection");

  await closePool(pool, brokers);
  is(pool.size, 0, "closing empties the pool");
  const after = await rejection(pool.acquire());
  is(after?.code, "ERR_RABBITMQ_CLOSED", "and a later borrow is refused");
  await pool.close();
  ok(true, "closing twice is harmless");
}

// -- a channel --------------------------------------------------------------

{
  const { dial, brokers } = dialer();
  const pool = new Pool(URL, { heartbeat: 0, dial, size: 1 });

  const using = pool.withChannel(async (channel) => {
    ok(channel.usable, "the borrowed channel is open");
    return "done";
  });
  const open = await brokers[0].expect("channel.open");
  await brokers[0].send("channel.open-ok", {}, open.channel);
  const close = await brokers[0].expect("channel.close");
  await brokers[0].send("channel.close-ok", {}, close.channel);

  is(await using, "done", "withChannel runs on a channel and returns its result");
  is(pool.size, 1, "and the connection is back in the pool");
  await closePool(pool, brokers);
}

// -- a size of one queues the second borrow ---------------------------------

{
  const { dial, brokers } = dialer();
  const pool = new Pool(URL, { heartbeat: 0, dial, size: 1 });

  const order = [];
  let open = () => {};
  const gate = new Promise((resolve) => {
    open = resolve;
  });

  const first = pool.withConnection(async () => {
    order.push("first");
    await gate;
  });
  const second = pool.withConnection(async () => {
    order.push("second");
  });

  await until(() => pool.busy === 1, "the first borrow to be lent");
  is(brokers.length, 1, "the second borrow waits rather than dialling past the size");
  is(pool.busy, 1, "with the one connection lent");

  open();
  await Promise.all([first, second]);
  is(order.join(","), "first,second", "it runs once the first has released");
  is(brokers.length, 1, "on the connection that was freed");

  await closePool(pool, brokers);
}

// -- recovery: a dead connection is replaced --------------------------------

{
  const { dial, brokers } = dialer();
  const opened = [];
  const pool = new Pool(URL, {
    heartbeat: 0,
    dial,
    size: 2,
    onConnect: (connection) => {
      opened.push(connection);
    },
  });

  const connection = await pool.acquire();
  is(opened.length, 1, "onConnect runs for the first connection");
  brokers[0].end();
  await until(() => !connection.usable, "the socket's death to be noticed");
  ok(!connection.usable, "a connection whose socket died is no longer usable");

  pool.release(connection);
  is(pool.size, 0, "releasing a dead connection drops it");
  is(pool.idle, 0, "it is not left idle");

  const replacement = await pool.acquire();
  ok(replacement.usable, "the next borrow dials a fresh connection");
  is(brokers.length, 2, "a second connection was dialled");
  is(opened.length, 2, "and onConnect ran for it too");
  is(pool.size, 1, "the pool holds only the replacement");
  pool.release(replacement);

  await closePool(pool, [brokers[1]]);
}

// -- a failing onConnect discards the connection ----------------------------

{
  const { dial, brokers } = dialer();
  let attempts = 0;
  const pool = new Pool(URL, {
    heartbeat: 0,
    dial,
    size: 1,
    onConnect: () => {
      attempts++;
      if (attempts === 1) throw new Error("redeclare failed");
    },
  });

  const borrowing = pool.withConnection(async () => "ran");
  await brokers[0].expect("connection.close");
  await brokers[0].send("connection.close-ok", {}, 0);

  const failure = await rejection(borrowing);
  ok(String(failure?.message).includes("redeclare failed"), "a failing onConnect fails the borrow");
  is(pool.size, 0, "and the connection is not kept");

  is(
    await pool.withConnection(async () => "ran"),
    "ran",
    "a later borrow dials again and succeeds",
  );
  is(brokers.length, 2, "with a fresh connection");
  await closePool(pool, [brokers[1]]);
}

// Closing while asynchronous setup is running must not lend its connection.
{
  const { dial, brokers } = dialer();
  let enterSetup;
  let finishSetup;
  const inSetup = new Promise((resolve) => {
    enterSetup = resolve;
  });
  const setup = new Promise((resolve) => {
    finishSetup = resolve;
  });
  const pool = new Pool(URL, {
    heartbeat: 0,
    dial,
    onConnect: async () => {
      enterSetup();
      await setup;
    },
  });

  const borrowing = pool.acquire();
  await inSetup;
  const closing = pool.close();
  await brokers[0].expect("connection.close");
  await brokers[0].send("connection.close-ok", {}, 0);
  await closing;
  finishSetup();
  const failure = await rejection(borrowing);
  is(
    failure?.code,
    "ERR_RABBITMQ_CLOSED",
    "a setup finishing after close cannot lend a connection",
  );
  is(pool.size, 0, "the closed pool retains no connection");
}

// -- a failed dial wakes the borrow queued behind it --------------------------

{
  let attempts = 0;
  const { dial: realDial, brokers } = dialer();
  const dial = async (address) => {
    attempts++;
    if (attempts === 1) throw new Error("refused");
    return realDial(address);
  };
  const pool = new Pool("amqp://broker.test/", { size: 1, dial, heartbeat: 0 });
  const first = rejection(pool.acquire());
  const second = pool.acquire();
  ok((await first) !== null, "the first borrow's failed dial is reported");
  const connection = await Promise.race([second, settle(500).then(() => null)]);
  ok(connection !== null, "and the borrow queued behind it dials instead of hanging");
  if (connection !== null) pool.release(connection);
  await closePool(pool, brokers);
}

// -- URL query parameters --------------------------------------------------

{
  const address = parseAmqpUrl(
    "amqps://h/v?heartbeat=10&channel_max=8&frame_max=65536&connection_timeout=2500&server_name_indication=broker.internal&auth_mechanism=plain&verify=verify_peer",
  );
  is(address.heartbeat, 10, "heartbeat is read from the query");
  is(address.channelMax, 8, "and channel_max");
  is(address.frameMax, 65536, "and frame_max");
  is(address.connectTimeout, 2500, "and connection_timeout");
  is(address.servername, "broker.internal", "and server_name_indication");
  is(parseAmqpUrl("amqp://[::1]:5672/").hostname, "::1", "an IPv6 literal loses its brackets");

  for (const [url, code] of [
    ["amqp://h/?hearbeat=10", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqps://h/?cacertfile=/etc/ca.pem", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqps://h/?certfile=c.pem", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqps://h/?verify=verify_none", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqp://h/?auth_mechanism=external", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqp://h/?server_name_indication=x", "ERR_RABBITMQ_UNSUPPORTED"],
    ["amqp://h/?heartbeat=abc", "ERR_RABBITMQ_CONNECTION"],
    ["amqp://h/?channel_max=70000", "ERR_RABBITMQ_CONNECTION"],
  ]) {
    let failure = null;
    try {
      parseAmqpUrl(url);
    } catch (e) {
      failure = e;
    }
    is(failure?.code ?? null, code, `${url} is refused, not ignored`);
  }

  // The query reaches the connection and the dial; settings still win.
  const { dial, brokers, addresses } = dialer();
  const connection = await openConnection(
    "amqps://broker.test/?heartbeat=0&frame_max=8192&connection_timeout=1234",
    { ca: "PEM", servername: "named.test", channelMax: 4 },
    dial,
  );
  is(connection.frameMax, 8192, "frame_max from the URL is what is proposed");
  is(addresses[0].connectTimeout, 1234, "connection_timeout bounds the dial");
  is(addresses[0].ca, "PEM", "the ca setting reaches the transport");
  is(addresses[0].servername, "named.test", "and so does servername");
  const defaults = dialer();
  const plain = await openConnection("amqp://broker.test/", { heartbeat: 0 }, defaults.dial);
  is(defaults.addresses[0].connectTimeout, 30000, "the dial is bounded by default");
  const closing = [connection.close(), plain.close()];
  for (const broker of [brokers[0], defaults.brokers[0]]) {
    await broker.expect("connection.close");
    await broker.send("connection.close-ok", {}, 0);
  }
  await Promise.all(closing);
}

if (report("pool") > 0) exit(1);
