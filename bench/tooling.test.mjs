// Regression checks for benchmark tooling; no performance measurements.
// Run: node --test bench/tooling.test.mjs
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:net";
import { setTimeout as delay } from "node:timers/promises";
import test from "node:test";

const read = (name) => readFileSync(new URL(name, import.meta.url), "utf8");

function temporary(fn) {
  const dir = mkdtempSync(join(tmpdir(), "es-bench-tooling-"));
  try {
    fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// Exercise the code used by the publisher rather than a second implementation.
const publisher = read("gen-bench-data.sh");
const merge = publisher.match(/bun -e '(\n  const fs = require\("fs"\);[\s\S]*?)' "\$TMP_COMBINED"/)[1];

for (const name of ["run_rps_hono", "run_rps_elysia", "run_rps_sustained"]) {
  test(`${name} publishes JSON even when text is requested by the caller`, () => {
    temporary((dir) => {
      const esdev = join(dir, "esdev");
      writeFileSync(esdev, "#!/bin/sh\nexit 0\n", { mode: 0o755 });
      const fn = name === "run_rps_hono"
        ? publisher.match(/run_rps_hono\(\) \{[^\n]+/)[0]
        : publisher.match(new RegExp(`${name}\\(\\) \\{[\\s\\S]*?\\n\\}`))[0];
      const result = spawnSync("bash", ["-c", `${fn}\nbash() { printf '%s' "$BENCH_RESPONSE_MODE"; }\n${name}`], {
        encoding: "utf8",
        env: { ...process.env, BENCH_RESPONSE_MODE: "text", ESDEV: esdev },
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "json");
    });
  });
}

for (const cached of [false, true]) {
  test(`scoped refresh preserves unmeasured rows (cached=${cached})`, () => {
    temporary((dir) => {
      const base = {
        rows: { edited: {}, kept: {} },
        results_ms: { edited: { esrun: 10, node: 12 }, kept: { esrun: 20 } },
        results_rss: { edited: { esrun: 30 }, kept: { esrun: 40 } },
        results_http2: { esrun: { narrow_h2: 100 } },
      };
      const patch = {
        rows: base.rows,
        results_ms: { edited: { esrun: 5 } },
        results_rss: { edited: { esrun: 25 } },
      };
      const existing = join(dir, "existing.js");
      const update = join(dir, "update.json");
      const kept = join(dir, "cached.json");
      const out = join(dir, "out.json");
      writeFileSync(existing, `export default ${JSON.stringify(base)}`);
      writeFileSync(update, JSON.stringify(patch));
      writeFileSync(kept, JSON.stringify({ results_ms: { edited: { esrun: 8 } } }));
      const fragments = cached ? [kept, update] : [update];
      const selection = publisher.slice(publisher.indexOf("OWNER_FRAGMENT="), publisher.indexOf("\nbun -e '", publisher.indexOf("OWNER_FRAGMENT=")));
      const owner = spawnSync("bash", ["-c", `${selection}\nprintf '%s' "$OWNER_FRAGMENT"`], {
        encoding: "utf8",
        env: { ...process.env, ROW_SCOPE: "edited", SECTIONS: "workloads", TMP1: kept, PATCHED_FULL: cached ? kept : "" },
      });
      assert.equal(owner.status, 0, owner.stderr);
      assert.equal(owner.stdout, "");
      const result = spawnSync(process.execPath, ["-e", merge, out, existing, owner.stdout, ...fragments], { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      const value = JSON.parse(readFileSync(out, "utf8"));
      assert.deepEqual(value.results_ms, { edited: { esrun: 5, node: 12 }, kept: { esrun: 20 } });
      assert.deepEqual(value.results_rss, { edited: { esrun: 25 }, kept: { esrun: 40 } });
      assert.deepEqual(value.results_http2, base.results_http2);
    });
  });
}

test("full refresh removes retired workload rows and preserves other sections", () => {
  temporary((dir) => {
    const existing = join(dir, "existing.js");
    const update = join(dir, "update.json");
    const out = join(dir, "out.json");
    writeFileSync(existing, 'export default {"results_ms":{"retired":{"esrun":20}},"results_http2":{"esrun":{"narrow_h2":100}}}');
    writeFileSync(update, '{"results_ms":{"current":{"esrun":5}},"rows":{"current":{}}}');
    const result = spawnSync(process.execPath, ["-e", merge, out, existing, update, update], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const value = JSON.parse(readFileSync(out, "utf8"));
    assert.deepEqual(value.results_ms, { current: { esrun: 5 } });
    assert.deepEqual(value.results_http2, { esrun: { narrow_h2: 100 } });
  });
});

for (const key of ["pg_qps_method", "mysql_qps_method"]) {
  test(`${key} replaces measured row metadata and preserves unmeasured rows`, () => {
    temporary((dir) => {
      const existing = join(dir, "existing.js");
      const update = join(dir, "update.json");
      const out = join(dir, "out.json");
      const kept = { measure_s: 10, aggregate: "max" };
      const fresh = { queries: 100000, reps: 3, aggregate: "max", spread_pct: { esrun: 2 } };
      writeFileSync(existing, `export default ${JSON.stringify({ [key]: { measured: kept, unmeasured: kept } })}`);
      writeFileSync(update, JSON.stringify({ [key]: { measured: fresh } }));
      const result = spawnSync(process.execPath, ["-e", merge, out, existing, "", update], { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(JSON.parse(readFileSync(out, "utf8"))[key], { measured: fresh, unmeasured: kept });
    });
  });
}

for (const name of ["rps.sh", "http2.sh"]) {
  for (const pin of ["", "env"]) {
    test(`${name} removes NO_COLOR with load prefix ${JSON.stringify(pin)}`, () => {
      temporary((dir) => {
        const tool = join(dir, "oha");
        const out = join(dir, "out.json");
        writeFileSync(tool, '#!/usr/bin/env python3\nimport json, os\nassert "NO_COLOR" not in os.environ\nprint(json.dumps({"summary":{"requestsPerSec":100,"average":0.001,"successRate":1},"statusCodeDistribution":{"200":1}}))\n', { mode: 0o755 });
        const load = read(name).match(/load\(\) \{[\s\S]*?\n\}/)[0];
        for (const duration of name === "rps.sh" ? ["", "1s"] : [""]) {
          const result = spawnSync("bash", ["-c", `${load}\nload h2 1 2`], {
            encoding: "utf8",
            env: { ...process.env, TOOL: "oha", OHA: tool, OUT: out, NO_COLOR: "1", LOAD_PIN: pin, REQUESTS: "10", CONN: "1", HDR: "Accept-Encoding: identity", URL: "http://localhost/", DURATION: duration },
          });
          assert.equal(result.status, 0, result.stderr);
          assert.equal(result.stdout.trim(), name === "rps.sh" ? "100 1.00" : "100");
        }
      });
    });
  }
}

test("generated dev fixture uses the current project configuration", () => {
  temporary((dir) => {
    const generator = join(dir, "gen.mjs");
    writeFileSync(generator, read("dev-server/gen.mjs"));
    const result = spawnSync(process.execPath, [generator, "11"], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const config = JSON.parse(readFileSync(join(dir, "apps/app-11/esdev.json"), "utf8"));
    assert.deepEqual(config, {
      build: { targets: { web: { entry: "index.html", outdir: "dist" } } },
      dev: { watch: { targets: ["web"] } },
    });
  });
});

test("launch samples reject a failing process instead of timing its refusal", () => {
  const runner = read("run.sh");
  const sample = runner.match(/sample_once\(\) \{[\s\S]*?\n\}/)[0];
  const helpers = runner.match(/^now\(\).*\n.*to_ms\(\).*$/m)[0];
  for (const [command, expected] of [["/bin/false", "ERR"], ["/bin/true", null]]) {
    const result = spawnSync("bash", ["-c", `${helpers}\n${sample}\nsample_once startup ${command} ignored.js`], {
      encoding: "utf8", env: { ...process.env, WRAP: "" },
    });
    assert.equal(result.status, 0, result.stderr);
    if (expected) assert.equal(result.stdout.trim(), expected);
    else assert.match(result.stdout.trim(), /^\d+\.\d+$/);
  }
});

test("generated launch fixtures are created within the benchmark sandbox", () => {
  temporary((dir) => {
    const runner = read("run.sh");
    const setup = runner.match(/mkdir -p \.cache\nSCRATCH="[^\n]+/)[0];
    const result = spawnSync("bash", ["-c", `${setup}\nprintf '%s' "$SCRATCH"`], {
      cwd: dir, encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.stdout.startsWith(`${dir}/.cache/`));
  });
});

for (const framework of ["hono", "elysia"]) {
  for (const mode of ["json", "text"]) {
    test(`${framework} serves the expected ${mode} response`, { timeout: 15000 }, async (t) => {
      const probe = createServer();
      await new Promise((resolve) => probe.listen(0, "127.0.0.1", resolve));
      const port = probe.address().port;
      await new Promise((resolve) => probe.close(resolve));
      const env = { ...process.env, BENCH_PORT: String(port) };
      if (mode === "json") delete env.BENCH_RESPONSE_MODE;
      else env.BENCH_RESPONSE_MODE = mode;
      const child = spawn(process.execPath, [new URL(`scripts/${framework}.js`, import.meta.url).pathname], { env, stdio: ["ignore", "ignore", "pipe"] });
      let errors = "";
      child.stderr.on("data", (data) => { errors += data; });
      t.after(async () => {
        if (child.exitCode === null && child.signalCode === null) {
          const done = new Promise((resolve) => child.once("exit", resolve));
          child.kill("SIGKILL");
          await done;
        }
      });
      let response;
      for (let attempt = 0; attempt < 100; attempt++) {
        assert.equal(child.exitCode, null, errors);
        try {
          response = await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(500) });
          break;
        } catch {
          await delay(50);
        }
      }
      assert.ok(response, `server did not listen: ${errors}`);
      assert.equal(response.status, 200);
      assert.equal(await response.text(), mode === "json" ? '{"message":"Hello, World!"}' : "Hello, World!");
      assert.ok(response.headers.get("content-type").includes(mode === "json" ? "application/json" : "text/plain"));
    });
  }
}
