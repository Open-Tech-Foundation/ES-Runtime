import { connect, DbErrorCode } from "runtime:db";
import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const listener = await connect(url, { driver: postgres });
const notifier = await connect(url, { driver: postgres });

const seen = [];
listener.onMessage = (payload, { channel }) => seen.push(`${channel}:${payload}`);

const perChannel = [];
await listener.subscribe("orders", (payload) => perChannel.push(payload));
await listener.subscribe("shipments");
console.log("channels:", listener.subscriptions.join(","), "| subscribed:", listener.subscribed);

// A subscribed connection is dedicated: it owns its reader, so it runs no
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
console.log("per-channel handler:", perChannel.join(","));

// Unsubscribing stops one channel and leaves the rest.
await listener.unsubscribe("orders");
console.log("after unsubscribe:", listener.subscriptions.join(","));
seen.length = 0;
await notifier.execute("NOTIFY orders, 'ignored'");
await notifier.execute("NOTIFY shipments, 'still here'");
await wait(1);
console.log("only shipments:", seen.join(" | "));

// A channel name is an identifier, so it is quoted rather than interpolated:
// this one would be a syntax error unquoted, and worse than that if it were
// chosen by someone else.
await listener.subscribe('weird "name"');
seen.length = 0;
await notifier.execute(`NOTIFY "weird ""name""", 'quoted'`);
await wait(1);
console.log("quoted identifier:", seen.join(" | "));

await listener.close();
await notifier.close();
