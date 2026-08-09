import "../dist/index.js";
import { env } from "runtime:process";
import { connect as netConnect, listen } from "runtime:net";
import { connect, DbErrorCode } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const target = new URL(url);

// A proxy we can cut. A server does not drop a connection on request, and the
// interesting case is not a polite FATAL from the backend — it is the socket
// simply ending, which is what a killed container or a dropped route looks
// like.
const proxy = listen({ hostname: "127.0.0.1", port: 0 });
const { port } = await proxy.addr;
const live = new Set();
(async () => {
  for await (const client of proxy) {
    const upstream = netConnect({ hostname: target.hostname, port: Number(target.port) });
    live.add(client);
    live.add(upstream);
    client.readable.pipeTo(upstream.writable).catch(() => {});
    upstream.readable.pipeTo(client.writable).catch(() => {});
  }
})().catch(() => {});

const through = `postgres://${target.username}:${target.password}@127.0.0.1:${port}${target.pathname}?sslmode=disable`;
const db = await connect(through);
console.log("connected through proxy:", (await (await db.query("SELECT 1 AS n")).first()).n);

// Cut every socket, then ask for something.
for (const socket of live) await socket.close().catch(() => {});

const first = await db.query("SELECT 2 AS n").then(
  () => "no error",
  (e) => e.code,
);
console.log("first after cut:", first === DbErrorCode.ConnectionLost);

// The second call gets the same answer rather than a different symptom of the
// same dead connection.
const second = await db.execute("SELECT 3").then(
  () => "no error",
  (e) => e.code,
);
console.log("second matches:", second === first);

// Closing a dead connection is not an error, and does not hang.
await db.close();
console.log("closed cleanly");
await proxy.close();
