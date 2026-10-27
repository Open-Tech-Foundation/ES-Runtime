// Stopping an operation: a signal or timeoutMS ends it and the client lives on;
// a pool at its size queues commands, and a queued command can be stopped too.
// Needs a server started with enableTestCommands, for `sleep`.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver, maxPoolSize: 2 });
const sleep = (millis, options) => client.command({ sleep: 1, millis }, options);

let started = Date.now();
let code = null;
try {
  await sleep(3000, { timeoutMS: 200 });
} catch (e) {
  code = e.code;
}
is(code, "ERR_DB_TIMEOUT", "timeoutMS stops a command");
ok(Date.now() - started < 1500, "promptly");

const controller = new AbortController();
setTimeout(() => controller.abort(new Error("enough")), 100);
started = Date.now();
let reason = null;
try {
  await sleep(3000, { signal: controller.signal });
} catch (e) {
  reason = e.message;
}
is(reason, "enough", "a signal rejects with its reason");
ok(Date.now() - started < 1500, "promptly");
await client.ping();
ok(true, "the client is still usable");

started = Date.now();
await Promise.all([sleep(300), sleep(300), sleep(300)]);
ok(Date.now() - started >= 550, "a third command waits for one of the pool's two connections");

const waiting = new AbortController();
const busy = [sleep(500), sleep(500)];
setTimeout(() => waiting.abort(new Error("gave up")), 50);
reason = null;
try {
  await client.ping({ signal: waiting.signal });
} catch (e) {
  reason = e.message;
}
is(reason, "gave up", "a command waiting for a connection can be stopped");
await Promise.all(busy);

const aborted = AbortSignal.abort(new Error("before"));
reason = null;
try {
  await client.ping({ signal: aborted });
} catch (e) {
  reason = e.message;
}
is(reason, "before", "an already-aborted signal stops the command before it is sent");
if (report("cancel") > 0) throw new Error("cancel failed");
