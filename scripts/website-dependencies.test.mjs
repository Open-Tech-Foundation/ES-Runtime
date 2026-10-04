// Run after pnpm install: node --test scripts/website-dependencies.test.mjs
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { test } from "node:test";

// Exercise the client actually resolved by the website's Miniflare dependency.
const websiteRequire = createRequire(new URL("../website/package.json", import.meta.url));
const wranglerRequire = createRequire(websiteRequire.resolve("wrangler"));
const miniflareRequire = createRequire(wranglerRequire.resolve("miniflare"));
const { MockAgent, interceptors, cacheStores } = miniflareRequire("undici");

test("shared HTTP caches do not replay another user's Set-Cookie response", async () => {
  const agent = new MockAgent();
  agent.disableNetConnect();
  const pool = agent.get("https://example.test");
  for (const user of ["alice", "bob"]) {
    pool.intercept({ path: "/session", method: "GET" }).reply(200, user, {
      headers: { "cache-control": "public, max-age=3600", "set-cookie": `user=${user}` },
    });
  }
  const client = agent.compose(interceptors.cache({ store: new cacheStores.MemoryCacheStore() }));
  try {
    for (const user of ["alice", "bob"]) {
      const response = await client.request({ origin: "https://example.test", path: "/session", method: "GET" });
      assert.equal(await response.body.text(), user);
      assert.equal(response.headers["set-cookie"], `user=${user}`);
    }
    agent.assertNoPendingInterceptors();
  } finally {
    await agent.close();
  }
});

test("HTTP caches reject unsafe methods in their configuration", () => {
  for (const method of ["POST", "PUT", "DELETE", "PATCH"]) {
    assert.throws(() => interceptors.cache({ methods: [method] }), /safe|method/i);
  }
});
