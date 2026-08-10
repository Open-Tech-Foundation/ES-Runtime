import { env } from "runtime:process";
import { connect } from "../dist/index.js";
import { DbErrorCode } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const listener = await connect(url);
const notifier = await connect(url);

const seen = [];
listener.onNotification = (n) => seen.push(`${n.channel}:${n.payload}`);

await listener.listen("orders");
await listener.listen("shipments");
console.log("channels:", listener.channels.join(","), "| listening:", listener.listening);

// A listening connection is dedicated: it owns its reader, so it runs no
// queries. Refused by name rather than deadlocking behind the read loop.
let refused = null;
try {
  await listener.query("SELECT 1");
} catch (e) {
  refused = e.code;
}
console.log("refuses queries:", refused === DbErrorCode.ConnectionBusy);

const wait = async (count, ms = 3000) => {
  const until = performance.now() + ms;
  while (seen.length < count && performance.now() < until) {
    await new Promise((r) => setTimeout(r, 20));
  }
};

await notifier.execute("NOTIFY orders, 'first'");
await notifier.execute("NOTIFY shipments, 'second'");
await notifier.execute("NOTIFY orders");
await wait(3);
console.log("received:", seen.join(" | "));

// Unlisten stops one channel and leaves the rest.
await listener.unlisten("orders");
console.log("after unlisten:", listener.channels.join(","));
seen.length = 0;
await notifier.execute("NOTIFY orders, 'ignored'");
await notifier.execute("NOTIFY shipments, 'still here'");
await wait(1);
console.log("only shipments:", seen.join(" | "));

// A channel name is an identifier, so it is quoted rather than interpolated:
// this one would be a syntax error unquoted, and worse than that if it were
// chosen by someone else.
await listener.listen('weird "name"');
seen.length = 0;
await notifier.execute(`NOTIFY "weird ""name""", 'quoted'`);
await wait(1);
console.log("quoted identifier:", seen.join(" | "));

await listener.close();
await notifier.close();
