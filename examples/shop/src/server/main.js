// The shop's server: the storefront's files, a JSON API over the durable
// workers, and a deliberately unreliable "fulfillment partner" for the
// webhooks to fail against. No database, cache or queue runs beside it.

import { file } from "runtime:fs";
import { serve } from "runtime:http";
import { env, exit, onSignal, unmask } from "runtime:process";
import { DurableError, configure, shutdown, startAlarms } from "runtime:workers";
import { PRODUCTS } from "../shared/catalog.js";
import { Customer, Delivery, Inventory } from "./workers.js";

const setting = (name, fallback) => unmask(env[name] ?? fallback);

const port = Number(setting("PORT", "8080"));
const shards = setting("SHARDS", "2");
// How often the fake partner refuses a webhook, so retries are visible.
const failRate = Number(setting("FAIL_RATE", "0.3"));
// The admin API is off unless a token is set, and then needs it as a bearer.
const adminToken = setting("ADMIN_TOKEN", "");

// The classes' code runs on shards; their state stays in this process. A shard
// imports the workers bundle, and needs `net` for the webhooks it sends.
configure({
  shards: shards === "auto" ? "auto" : Number(shards),
  module: new URL("./workers.js", import.meta.url),
  permissions: ["net"],
});

const alarms = startAlarms({
  classes: [Customer, Delivery],
  onError: (error, context) => console.error(`alarm: ${context}:`, error),
});

const publicDir = new URL("./public/", import.meta.url);

const TYPES = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  svg: "image/svg+xml",
  json: "application/json",
  map: "application/json",
};

async function asset(pathname) {
  const name = pathname === "/" ? "index.html" : pathname.slice(1);
  if (name.split("/").includes("..")) return new Response("not found", { status: 404 });
  let handle = file(new URL(name, publicDir));
  // A client-side route is answered with the page, as any SPA host does.
  if (!(await handle.exists())) handle = file(new URL("index.html", publicDir));
  const ext = handle.path.split(".").pop() ?? "";
  return new Response(handle.stream(), {
    headers: { "content-type": TYPES[ext] ?? "application/octet-stream" },
  });
}

function session(request) {
  const cookie = request.headers.get("cookie") ?? "";
  const match = /(?:^|;\s*)sid=([0-9a-f-]{36})/.exec(cookie);
  return match?.[1] ? { id: match[1], fresh: false } : { id: crypto.randomUUID(), fresh: true };
}

// A durable worker's failures have codes; the API turns the ones a client can
// act on into statuses rather than a 500.
function failure(error) {
  if (error instanceof DurableError || error?.code?.startsWith("ERR_DURABLE")) {
    const status = error.code === "ERR_DURABLE_BUSY" || error.code === "ERR_DURABLE_SHARD_LOST" ? 503 : 500;
    return Response.json({ error: error.message, code: error.code }, { status });
  }
  if (error instanceof RangeError || error instanceof TypeError) {
    return Response.json({ error: error.message }, { status: 400 });
  }
  console.error(error);
  return Response.json({ error: "internal error" }, { status: 500 });
}

async function api(request, url, sid) {
  const route = `${request.method} ${url.pathname}`;
  const customer = Customer.get(sid);
  switch (route) {
    case "GET /api/products": {
      const available = await Promise.all(PRODUCTS.map((p) => Inventory.get(p.id).available()));
      return Response.json(PRODUCTS.map((p, i) => ({ ...p, available: available[i] })));
    }
    case "GET /api/cart":
      return Response.json(await customer.view());
    case "POST /api/cart": {
      const { id, qty } = await request.json();
      return Response.json(await customer.setQty(String(id), Number(qty)));
    }
    case "POST /api/checkout":
      return Response.json(await customer.checkout(new URL("/fulfillment", url).href));
    case "GET /api/orders":
      return Response.json(await customer.orders());
    case "POST /api/admin/restock": {
      if (!adminToken || request.headers.get("authorization") !== `Bearer ${adminToken}`) {
        return Response.json({ error: "forbidden" }, { status: 403 });
      }
      const { id, qty } = await request.json();
      if (!PRODUCTS.some((p) => p.id === id)) return Response.json({ error: "no such product" }, { status: 400 });
      return Response.json({ id, available: await Inventory.get(id).restock(Number(qty)) });
    }
    default:
      return Response.json({ error: "not found" }, { status: 404 });
  }
}

// The partner. It refuses some deliveries outright, which is what the
// webhook's retries are for, and it counts what it has seen by idempotency key
// so a repeat is visible in the log rather than double-shipped.
const shipped = new Set();

async function fulfillment(request) {
  const key = request.headers.get("idempotency-key") ?? "";
  await request.body?.cancel();
  if (Math.random() < failRate) return new Response("partner unavailable", { status: 503 });
  console.log(shipped.has(key) ? `fulfillment: repeat of ${key}` : `fulfillment: shipped ${key}`);
  shipped.add(key);
  return new Response(null, { status: 204 });
}

const server = serve({ port }, async (request) => {
  const url = new URL(request.url);
  if (url.pathname === "/fulfillment" && request.method === "POST") return fulfillment(request);
  if (!url.pathname.startsWith("/api/")) return asset(url.pathname);
  const { id, fresh } = session(request);
  let response;
  try {
    response = await api(request, url, id);
  } catch (error) {
    response = failure(error);
  }
  if (fresh) {
    response.headers.append("set-cookie", `sid=${id}; Path=/; HttpOnly; SameSite=Lax; Max-Age=31536000`);
  }
  return response;
});

console.log(`shop on http://localhost:${(await server.addr).port} (${shards} shards)`);

// A stop that finishes what it was doing: requests drain, then every worker is
// flushed and closed, and its `stop()` runs.
onSignal("SIGTERM", async () => {
  await server.stop();
  await alarms.stop();
  await shutdown();
  exit(143);
});
