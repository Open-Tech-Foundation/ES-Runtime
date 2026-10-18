// A promise must not be left to reject on its own, with nobody listening.
//
// This file deliberately does not call `exit()`: esrun reports an unhandled
// rejection as fatal only once the event loop drains, and `exit()` bypasses that
// report. So the file drains and the runtime — not an assertion — is what catches
// an abandoned rejection. Two ways one can be abandoned are covered, each of
// which leaves the connection up, so the connection's own cleanup never runs and
// the client has to handle it:
//
//   1. a publish that cannot be encoded — nothing reaches the socket; and
//   2. a waiter whose timer fires before its `send` finishes — the write stalls
//      past the timeout, so the acknowledgement waiter rejects while `publish`,
//      `subscribe` and `unsubscribe` are all still awaiting the socket.
import { MqttClient } from "../../dist/client.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

// A client whose writes can be stalled: after `stall()`, every write waits
// forever, so a waiter's timer can fire while `send` is still pending.
async function connected() {
  const broker = fakeBroker(5);
  let stalled = false;
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => (stalled ? new Promise(() => {}) : broker.transport.write(bytes)),
    close: () => broker.transport.close(),
  };
  const pending = MqttClient.over(transport, { clientId: "c", keepAlive: 0 }, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const client = await pending;
  return {
    broker,
    client,
    stall() {
      stalled = true;
    },
  };
}

// --- a local encoding failure ----------------------------------------------

{
  const { broker, client } = await connected();

  let error = null;
  try {
    // U+0000 is refused in a UTF-8 Encoded String, so this fails while encoding,
    // before anything reaches the socket.
    await client.publish("a/b", "x", { qos: 1, timeout: 60, contentType: "a\u0000b" });
  } catch (e) {
    error = e;
  }
  ok(error !== null, "a publish that cannot be encoded fails");

  // Hold the loop past the waiter's timeout so a leaked timer would fire while
  // the script is still alive — the whole point of draining rather than exiting.
  await new Promise((resolve) => setTimeout(resolve, 150));

  // The failure was local, so the connection is untouched and still usable.
  await client.publish("a/b", "ok");
  const packet = await broker.next();
  ok(packet.type === PacketType.Publish, "the client is still usable afterwards");
  await client.close();
}

// --- a write that outlasts the timeout -------------------------------------

// The waiter's timer is armed before the packet is written, so a write slower
// than the timeout rejects the waiter while the caller is still awaiting `send`.
// A rejection left for the caller's later `await` is fine; one left with nobody
// awaiting it ends this script. All three request/acknowledge paths are covered.
for (const [name, call] of [
  ["publish", (client) => client.publish("a/b", "x", { qos: 1, timeout: 60 })],
  ["subscribe", (client) => client.subscribe("a/#", { timeout: 60 })],
  ["unsubscribe", (client) => client.unsubscribe("a/#", 60)],
]) {
  const { client, stall } = await connected();
  stall();
  void call(client);
  await new Promise((resolve) => setTimeout(resolve, 150));
  ok(true, `${name}: a stalled write leaves no unhandled rejection`);
}

if (report("unhandled") > 0) throw new Error("checks failed");
