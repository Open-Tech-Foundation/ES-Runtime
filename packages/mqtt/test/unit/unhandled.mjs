// A promise must not be left to reject on its own, with nobody listening.
//
// This file deliberately does not call `exit()`: esrun reports an unhandled
// rejection as fatal only once the event loop drains, and `exit()` bypasses that
// report. So the file drains and the runtime — not an assertion — is what catches
// an abandoned rejection.
//
// The case here is a publish that cannot be encoded: nothing reaches the socket,
// so the connection stays up and its own cleanup never runs, and the client has
// to withdraw the acknowledgement waiter itself. A write that stalls past the
// timeout is the related case; that one settles rather than hangs, and is
// asserted directly in client.mjs.
import { MqttClient } from "../../dist/client.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const broker = fakeBroker(5);
const pending = MqttClient.over(broker.transport, { clientId: "c", keepAlive: 0 }, 5);
await broker.next();
broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
const client = await pending;

let error = null;
try {
  // U+0000 is refused in a UTF-8 Encoded String, so this fails while encoding,
  // before anything reaches the socket.
  await client.publish("a/b", "x", { qos: 1, timeout: 60, contentType: "a\u0000b" });
} catch (e) {
  error = e;
}
ok(error !== null, "a publish that cannot be encoded fails");

// Hold the loop past the waiter's timeout so a leaked timer would fire while the
// script is still alive — the whole point of draining rather than exiting.
await new Promise((resolve) => setTimeout(resolve, 150));

// The failure was local, so the connection is untouched and still usable.
await client.publish("a/b", "ok");
const packet = await broker.next();
ok(packet.type === PacketType.Publish, "the client is still usable afterwards");
await client.close();

if (report("unhandled") > 0) throw new Error("checks failed");
