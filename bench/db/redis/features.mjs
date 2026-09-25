// What each Redis client actually does, for the feature table on the comparison
// page (website/app/docs/comparison) and the Redis guide.
//
// Every row is a probe against a live server, not a reading of documentation:
// a feature is "yes" because a call through the client's own API did it. Rows
// that need infrastructure a single server cannot provide — a cluster, a
// Sentinel, a private CA — report whether the client has the API, and say so.
//
//   node bench/db/redis/features.mjs node-redis
//   node bench/db/redis/features.mjs ioredis
//   bun  bench/db/redis/features.mjs bun
//   esrun --allow-net --allow-imports --allow-env bench/db/redis/features.mjs esrun
//
// REDIS_FEATURES_URL picks the server (default redis://127.0.0.1:6379). The
// script writes and deletes keys under `features:`. esrun needs the driver
// staged beside it as `.driver/` (bench/db/redis/run.sh does the same).

const client = (globalThis.Bun?.argv ?? globalThis.process?.argv ?? [])[2] ?? (await esrunArg());
const url = (await envOf("REDIS_FEATURES_URL")) ?? "redis://127.0.0.1:6379";

async function esrunArg() {
  const { args } = await import("runtime:process");
  return args[0];
}
async function envOf(name) {
  if (globalThis.process?.env) return process.env[name];
  const { env } = await import("runtime:process");
  return env[name];
}

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
/** Resolves with the promise's value, or `timeout` if it takes longer than `ms`. */
const within = (promise, ms) =>
  Promise.race([promise, wait(ms).then(() => "timeout")]);
const EXACT = "9007199254740993"; // 2^53 + 1: the first integer a double cannot hold

// --- adapters: the same operations, through each client's documented API -----

const adapters = {
  async esrun() {
    const { connect, DbErrorCode } = await import("runtime:db");
    const { driver, redisCluster, redisSentinel } = await import("./.driver/index.js");
    const open = (options = {}) => connect(url, { driver, ...options });
    return {
      open: async () => {
        const r = await open();
        return {
          raw: (args) => r.call(args),
          pipeline: typeof r.pipeline === "function",
          multi: typeof r.multi === "function",
          watch: typeof r.watch === "function",
          streams: typeof r.xadd === "function" && typeof r.xreadgroup === "function",
          close: () => r.close(),
        };
      },
      binaryGet: async (key) => (await open({ binary: true })).call(["GET", key]),
      subscribe: async (channel, onMessage) => (await open()).subscribe(channel, onMessage),
      portableCode: (error) => Object.values(DbErrorCode).includes(error?.code),
      cluster: typeof redisCluster === "object",
      sentinel: typeof redisSentinel === "object",
    };
  },

  async "node-redis"() {
    const { createClient, createCluster, createSentinel, RESP_TYPES } = await import("redis");
    const open = async () => {
      const c = createClient({ url });
      c.on("error", () => {});
      await c.connect();
      return c;
    };
    return {
      open: async () => {
        const c = await open();
        return {
          raw: (args) => c.sendCommand(args),
          pipeline: typeof c.pipeline === "function",
          multi: typeof c.multi === "function",
          watch: typeof c.watch === "function",
          streams: typeof c.xAdd === "function" && typeof c.xReadGroup === "function",
          close: () => c.destroy(),
        };
      },
      binaryGet: async (key) =>
        (await open()).withTypeMapping({ [RESP_TYPES.BLOB_STRING]: Buffer }).get(key),
      subscribe: async (channel, onMessage) => (await open()).subscribe(channel, onMessage),
      portableCode: (error) => typeof error?.code === "string" && !/^[A-Z]+$/.test(error.code),
      cluster: typeof createCluster === "function",
      sentinel: typeof createSentinel === "function",
    };
  },

  async ioredis() {
    const { default: Redis, Cluster } = await import("ioredis");
    const open = () => {
      const c = new Redis(url);
      c.on("error", () => {});
      return c;
    };
    return {
      open: async () => {
        const c = open();
        return {
          raw: ([command, ...rest]) => c.call(command, ...rest),
          pipeline: typeof c.pipeline === "function",
          multi: typeof c.multi === "function",
          watch: typeof c.watch === "function",
          streams: typeof c.xadd === "function" && typeof c.xreadgroup === "function",
          close: () => c.disconnect(),
        };
      },
      binaryGet: (key) => open().getBuffer(key),
      subscribe: async (channel, onMessage) => {
        const sub = open();
        sub.on("message", (_channel, message) => onMessage(message));
        await sub.subscribe(channel);
      },
      portableCode: (error) => typeof error?.code === "string" && !/^[A-Z]+$/.test(error.code),
      cluster: typeof Cluster === "function",
      // Sentinel is an option of the ordinary client (`sentinels: [...]`).
      sentinel: true,
    };
  },

  async bun() {
    const { RedisClient } = await import("bun");
    const open = async () => {
      const c = new RedisClient(url);
      await c.connect();
      return c;
    };
    return {
      open: async () => {
        const c = await open();
        return {
          raw: ([command, ...rest]) => c.send(command, rest),
          // Pipelining is automatic; there is no builder to hand commands to.
          pipeline: false,
          multi: typeof c.multi === "function",
          watch: typeof c.watch === "function",
          streams: typeof c.xadd === "function" && typeof c.xreadgroup === "function",
          close: () => c.close(),
        };
      },
      binaryGet: async (key) => (await open()).getBuffer(key),
      subscribe: async (channel, onMessage) => (await open()).subscribe(channel, onMessage),
      // Bun's codes are its own (ERR_REDIS_*), not shared with its SQL clients.
      portableCode: () => false,
      cluster: false,
      sentinel: false,
    };
  },
};

// --- the probes ---------------------------------------------------------------

const make = adapters[client];
if (!make) throw new Error(`unknown client "${client}" — one of: ${Object.keys(adapters).join(", ")}`);
const a = await make();
const c = await a.open();
const admin = await a.open();
const result = { client };

// RESP3 negotiated by default: what the server says this connection speaks.
const info = String(await c.raw(["CLIENT", "INFO"]));
result.resp3ByDefault = /\bresp=3\b/.test(info);

// Exact 64-bit integers.
await c.raw(["DEL", "features:int"]);
const n = await c.raw(["INCRBY", "features:int", EXACT]);
result.exact64 = String(n) === EXACT;

// Binary-safe values: bytes that are not UTF-8 come back as the same bytes.
await admin.raw(["DEL", "features:bin"]);
await c.raw(["SET", "features:bin", "x"]); // created here, overwritten below
const bytes = new Uint8Array([0xff, 0x00, 0xfe, 0x80]);
await (typeof Buffer === "function"
  ? c.raw(["SET", "features:bin", Buffer.from(bytes)])
  : c.raw(["SET", "features:bin", bytes]));
const back = await a.binaryGet("features:bin");
result.binarySafe = back != null && [...new Uint8Array(back)].join() === [...bytes].join();

// Pub/sub.
let received = null;
await a.subscribe("features:ch", (message) => (received = String(message)));
await wait(100);
await c.raw(["PUBLISH", "features:ch", "hello"]);
await within(new Promise((resolve) => { const t = setInterval(() => received && (clearInterval(t), resolve()), 10); }), 2000);
result.pubsub = received === "hello";

result.pipelineBuilder = c.pipeline;
result.multiApi = c.multi;
result.watchApi = c.watch;
result.streamsTyped = c.streams;
result.clusterApi = a.cluster;
result.sentinelApi = a.sentinel;

// Portable error codes: an error the application can branch on without
// knowing which database raised it.
try {
  await c.raw(["AUTH", "features-no-such-user", "wrong"]);
  result.portableErrors = false;
} catch (error) {
  result.portableErrors = a.portableCode(error);
  result.errorCode = error?.code ?? null;
}

// Unbounded-block guard: a blocking pop with no timeout, on an ordinary
// connection, is refused rather than left to hold it for ever.
const blocked = c.raw(["BLPOP", "features:q", "0"]).then(
  () => "returned",
  () => "refused",
);
const outcome = await within(blocked, 1000);
result.blockGuard = outcome === "refused";
if (outcome === "timeout") {
  await admin.raw(["LPUSH", "features:q", "unblock"]);
  await within(blocked, 2000);
}

// Auto-reconnect, with the client's defaults: the server drops the connection,
// and the next command on it works without the application reopening it.
await admin.raw(["CLIENT", "KILL", "TYPE", "normal", "SKIPME", "yes"]);
await wait(300);
const afterKill = await within(
  c.raw(["PING"]).then(() => "ok", () => "failed"),
  5000,
);
result.autoReconnect = afterKill === "ok";

await within(admin.raw(["DEL", "features:int", "features:bin", "features:q"]), 2000).catch(() => {});
console.log(JSON.stringify(result));
await within(Promise.all([c.close?.(), admin.close?.()]).catch(() => {}), 1000);
// The subscriber and the binary connections are still open, which keeps any
// runtime alive; the measurement is done.
if (globalThis.process?.exit) process.exit(0);
else (await import("runtime:process")).exit(0);
