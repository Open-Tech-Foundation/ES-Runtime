// The shop example under load, killed, and raced (examples/shop).
//
// Run from the repository root:
//
//     esdev bench/durable-shop.js
//
//     ESDEV=target/release/esdev   which esdev runs the server (default: esdev)
//     CUSTOMERS=32                 concurrent customers
//     SECONDS=10                   how long each load run lasts
//     SHARDS="0 2 4"               the pools to compare
//     WORKDIR=/tmp/shop            where the server runs and keeps its state
//                                  (default: examples/shop itself)
//
// **Say which disk.** A durable write is a commit, and a commit is an fsync:
// on a spinning disk that is ~10 ms, on an SSD or tmpfs well under 1 ms, and
// the shop's numbers move by the same factor. Set WORKDIR to compare.
//
// Every number the site quotes about the shop comes from here. Four questions:
//
//   * **What does a customer's journey cost?** Browse, two adds, checkout and
//     the order history, with a new customer each time — so every journey
//     materializes a fresh worker, as real traffic does. Per-route p50/p99,
//     journeys per second, and the server's resident memory, for each pool.
//   * **Is an acknowledged order ever lost?** The server is killed with SIGKILL
//     in the middle of the load, restarted on the same state, and every order
//     a customer was told about is looked for.
//   * **Does an interrupted checkout finish?** Checkouts in flight at the kill
//     either completed before it or are resumed by the customer's `start()`.
//   * **Is the last item ever sold twice?** Forty customers race for a
//     product with five in stock.
//
// The client is one process driving every customer, so at high concurrency it
// can be the bottleneck; the p99 is measured end to end and says so.

import { env, exit, unmask } from "runtime:process";
import { Command } from "runtime:system";

const setting = (name, fallback) => unmask(env[name] ?? fallback);

const ESDEV = setting("ESDEV", "esdev");
const CUSTOMERS = Number(setting("CUSTOMERS", "32"));
const SECONDS = Number(setting("SECONDS", "10"));
const POOLS = setting("SHARDS", "0 2 4").split(/\s+/).filter(Boolean).map(Number);

const SHOP = new URL("../examples/shop/", import.meta.url);
// The server runs from WORKDIR, and keeps its state in `WORKDIR/.durable` —
// `../.durable` from `dist/server.js`. Outside this script's sandbox, so it is
// copied and cleared with `cp` and `rm` rather than `runtime:fs`.
const trim = (path) => path.replace(/\/+$/, "");
const IN_PLACE = trim(SHOP.pathname);
const WORKDIR = trim(setting("WORKDIR", "") || IN_PLACE);
const STATE = `${WORKDIR}/.durable`;
const PORT = 8091;
const BASE = `http://127.0.0.1:${PORT}`;
const TOKEN = "bench";
const PRODUCTS = ["mug", "tee", "cap", "tote", "pin", "hoodie"];

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// ---------------------------------------------------------------------------
// The server
// ---------------------------------------------------------------------------

async function build() {
  const { success, stderr } = await new Command(ESDEV, {
    args: ["build", "--minify"],
    cwd: SHOP,
    inheritEnv: true,
  }).output();
  if (!success) throw new Error(`esdev build failed:\n${new TextDecoder().decode(stderr)}`);
  if (WORKDIR !== IN_PLACE) {
    await sh("mkdir", "-p", WORKDIR);
    await sh("rm", "-rf", `${WORKDIR}/dist`);
    await sh("cp", "-r", new URL("dist", SHOP).pathname, `${WORKDIR}/dist`);
  }
}

async function sh(program, ...args) {
  const { success, stderr } = await new Command(program, { args }).output();
  if (!success) throw new Error(`${program} ${args.join(" ")}: ${new TextDecoder().decode(stderr)}`);
}

const fresh = () => sh("rm", "-rf", STATE);

async function start(shards) {
  const started = performance.now();
  const child = await new Command(ESDEV, {
    args: ["dist/server.js"],
    cwd: WORKDIR,
    inheritEnv: true,
    env: { PORT: String(PORT), SHARDS: String(shards), FAIL_RATE: "0", ADMIN_TOKEN: TOKEN },
    stdout: "null",
    stderr: "inherit",
  }).spawn();
  for (let i = 0; i < 300; i++) {
    try {
      const res = await fetch(`${BASE}/api/products`);
      await res.body?.cancel();
      if (res.ok) return { child, ready: performance.now() - started };
    } catch {
      // Not listening yet.
    }
    await sleep(50);
  }
  await child.kill("SIGKILL");
  throw new Error("the shop did not start");
}

async function stop(server, signal = "SIGTERM") {
  await server.child.kill(signal);
  await server.child.status;
}

// Resident memory of the server, as `ps` reports it. `/proc` is outside the
// sandbox, which is the working directory.
async function rss(pid) {
  try {
    const { success, stdout } = await new Command("ps", { args: ["-o", "rss=", "-p", String(pid)] }).output();
    const kb = Number(new TextDecoder().decode(stdout).trim());
    return success && kb > 0 ? `${(kb / 1024).toFixed(0)} MB` : "n/a";
  } catch {
    return "n/a";
  }
}

async function restock(qty) {
  for (const id of PRODUCTS) {
    const res = await fetch(`${BASE}/api/admin/restock`, {
      method: "POST",
      headers: { authorization: `Bearer ${TOKEN}`, "content-type": "application/json" },
      body: JSON.stringify({ id, qty }),
    });
    if (!res.ok) throw new Error(`restock ${id}: ${res.status}`);
    await res.body?.cancel();
  }
}

// ---------------------------------------------------------------------------
// A customer
// ---------------------------------------------------------------------------

class Customer {
  sid = null;

  constructor(stats) {
    this.stats = stats;
  }

  async request(route, method, path, body) {
    const started = performance.now();
    const res = await fetch(`${BASE}${path}`, {
      method,
      headers: {
        ...(this.sid ? { cookie: `sid=${this.sid}` } : {}),
        ...(body === undefined ? {} : { "content-type": "application/json" }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const data = await res.json();
    this.stats?.record(route, performance.now() - started, res.status);
    const cookie = res.headers.get("set-cookie");
    if (cookie) this.sid = /sid=([^;]+)/.exec(cookie)?.[1] ?? this.sid;
    if (!res.ok) throw Object.assign(new Error(data.error ?? res.status), { status: res.status });
    return data;
  }

  products = () => this.request("products", "GET", "/api/products");
  add = (id, qty) => this.request("add", "POST", "/api/cart", { id, qty });
  checkout = () => this.request("checkout", "POST", "/api/checkout");
  orders = () => this.request("orders", "GET", "/api/orders");
}

class Stats {
  routes = new Map();
  errors = new Map();
  journeys = 0;

  record(route, ms, status) {
    if (status >= 400) this.errors.set(status, (this.errors.get(status) ?? 0) + 1);
    let list = this.routes.get(route);
    if (!list) this.routes.set(route, (list = []));
    list.push(ms);
  }

  percentile(route, p) {
    const list = [...(this.routes.get(route) ?? [])].sort((a, b) => a - b);
    if (list.length === 0) return NaN;
    return list[Math.min(list.length - 1, Math.floor((p / 100) * list.length))];
  }
}

// One journey: a new customer browses, adds two products and checks out.
async function journey(stats, ledger) {
  const c = new Customer(stats);
  await c.products();
  const a = PRODUCTS[Math.floor(Math.random() * PRODUCTS.length)];
  const b = PRODUCTS[(PRODUCTS.indexOf(a) + 1) % PRODUCTS.length];
  await c.add(a, 1);
  await c.add(b, 2);
  ledger?.inFlight.add(c.sid);
  const order = await c.checkout();
  ledger?.inFlight.delete(c.sid);
  ledger?.acked.set(c.sid, order.id);
  await c.orders();
  stats.journeys++;
}

async function load(seconds, customers, ledger) {
  const stats = new Stats();
  const deadline = performance.now() + seconds * 1000;
  let stopped = false;
  const runner = async () => {
    while (!stopped && performance.now() < deadline) {
      try {
        await journey(stats, ledger);
      } catch (e) {
        // Counted by status; a refused connection (the kill) ends the runner.
        if (e.status === undefined) {
          if (ledger) return;
          stats.errors.set("network", (stats.errors.get("network") ?? 0) + 1);
        }
      }
    }
  };
  const started = performance.now();
  const runners = Array.from({ length: customers }, runner);
  return {
    stats,
    stop: () => {
      stopped = true;
    },
    done: Promise.all(runners).then(() => ({ stats, elapsed: (performance.now() - started) / 1000 })),
  };
}

// ---------------------------------------------------------------------------
// The scenarios
// ---------------------------------------------------------------------------

const ms = (n) => (Number.isFinite(n) ? `${n.toFixed(1)} ms` : "—");

async function throughput(shards) {
  await fresh();
  const server = await start(shards);
  await restock(1_000_000);
  // A short warm-up, so the numbers are the steady state rather than the JIT.
  await (await load(1, CUSTOMERS)).done;
  const run = await load(SECONDS, CUSTOMERS);
  const { stats, elapsed } = await run.done;
  const memory = await rss(server.child.pid);
  await stop(server);
  return { shards, stats, elapsed, memory, ready: server.ready };
}

async function crash() {
  await fresh();
  let server = await start(2);
  await restock(1_000_000);
  const ledger = { acked: new Map(), inFlight: new Set() };
  const run = await load(120, CUSTOMERS, ledger);
  // Killed once there is something to lose: enough acknowledged orders, or a
  // minute, whichever comes first — a slow disk takes a while to get there.
  const until = performance.now() + 60_000;
  while (ledger.acked.size < 100 && performance.now() < until) await sleep(100);
  await stop(server, "SIGKILL");
  run.stop();
  await run.done;
  const inFlight = [...ledger.inFlight];

  server = await start(2);
  const restart = server.ready;
  let lost = 0;
  let resumed = 0;
  for (const [sid, orderId] of ledger.acked) {
    const c = new Customer();
    c.sid = sid;
    const orders = await c.orders();
    if (!orders.some((o) => o.id === orderId)) lost++;
  }
  for (const sid of inFlight) {
    const c = new Customer();
    c.sid = sid;
    if ((await c.orders()).length > 0) resumed++;
  }
  // Every acknowledged order's webhook is delivered by alarms that outlived
  // the kill.
  let undelivered = ledger.acked.size;
  const deadline = performance.now() + 15_000;
  while (undelivered > 0 && performance.now() < deadline) {
    // The server's scheduler does the delivering; this only gives it room.
    await sleep(500);
    undelivered = 0;
    for (const sid of ledger.acked.keys()) {
      const c = new Customer();
      c.sid = sid;
      if ((await c.orders()).some((o) => o.delivery !== "delivered")) undelivered++;
    }
  }
  await stop(server);
  return { acked: ledger.acked.size, lost, inFlight: inFlight.length, resumed, restart, undelivered };
}

async function contention() {
  await fresh();
  const server = await start(2);
  const racers = Array.from({ length: 40 }, async () => {
    const c = new Customer();
    const { granted } = await c.add("hoodie", 1);
    if (granted === 0) return 0;
    const order = await c.checkout();
    return order.lines.find((l) => l.id === "hoodie")?.qty ?? 0;
  });
  const sold = (await Promise.all(racers)).reduce((a, b) => a + b, 0);
  const left = (await new Customer().products()).find((p) => p.id === "hoodie").available;
  await stop(server);
  return { sold, left };
}

// ---------------------------------------------------------------------------

console.log(`building examples/shop with ${ESDEV}…`);
await build();

console.log(`\nstate in ${STATE}`);
console.log(`\n## Load — ${CUSTOMERS} concurrent customers, ${SECONDS}s, a new customer per journey\n`);
console.log("| Shards | Journeys/s | Browse p50 / p99 | Add p50 / p99 | Checkout p50 / p99 | Errors | Server RSS | Start |");
console.log("| --- | --- | --- | --- | --- | --- | --- | --- |");
for (const shards of POOLS) {
  const { stats, elapsed, memory, ready } = await throughput(shards);
  const pair = (route) => `${ms(stats.percentile(route, 50))} / ${ms(stats.percentile(route, 99))}`;
  const errors = [...stats.errors].map(([k, v]) => `${k}×${v}`).join(" ") || "0";
  console.log(
    `| ${shards} | ${(stats.journeys / elapsed).toFixed(0)} | ${pair("products")} | ${pair("add")} | ` +
      `${pair("checkout")} | ${errors} | ${memory} | ${ms(ready)} |`,
  );
}

console.log("\n## SIGKILL mid-load, then restart on the same state (2 shards)\n");
const k = await crash();
console.log("| Acknowledged orders | Lost | Checkouts in flight at the kill | Finished after restart | Undelivered webhooks | Restart |");
console.log("| --- | --- | --- | --- | --- | --- |");
console.log(`| ${k.acked} | ${k.lost} | ${k.inFlight} | ${k.resumed} | ${k.undelivered} | ${ms(k.restart)} |`);

console.log("\n## 40 customers race for 5 hoodies\n");
const c = await contention();
console.log(`| Sold | Left | Oversold |\n| --- | --- | --- |\n| ${c.sold} | ${c.left} | ${c.sold > 5 ? "yes" : "no"} |`);

await fresh();
if (k.lost > 0 || c.sold > 5) {
  console.error("\nFAILED: an acknowledged order was lost, or stock was oversold");
  exit(1);
}
