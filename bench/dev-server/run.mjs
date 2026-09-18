// Dev-server benchmark: vite vs oj vs esdev start on the generated fixture
// (bench/dev-server/apps/app-<N>, same fanout-10 shape as oj's bench).
//
// Methodology follows oj's bench/run.mjs, minus the reload/HMR legs: per tool,
// ITERS sessions of cold (tool caches cleared) + warm (caches primed by the
// cold run); each session measures spawn -> server-ready PLUS a full browser
// render to [data-done], because for an on-demand dev server "ready" fires
// before any module is transformed and the render is the cost. Memory is the
// server's peak RSS after the first cold render. Published number per cell is
// the MIN over sessions (repo convention: contention only adds time).
//
// Legs: `vite` (default dev), `oj dev --bundle` (the mode oj's site charts
// for the 10k-component app), `esdev start` (the dev loop), `bun ./index.html`
// (Bun's zero-config frontend serve). One browser drives every render;
// servers are killed between sessions.
//
// Usage:
//   node gen.mjs 10000 && (cd apps/app-10000 && npm install)
//   node run.mjs [N]                    human table
//   BENCH_JSON=1 node run.mjs [N]       machine JSON for gen-bench-data.sh
//
// Knobs: BENCH_ITERS (default 3), CHROME_PATH, OJ_BIN, ESDEV_BIN.
// Requires: oj on PATH, esdev built, a Chrome/Chromium binary.
import { spawn, execSync, execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";

const N = parseInt(process.argv[2] ?? "10000", 10);
const ITERS = parseInt(process.env.BENCH_ITERS ?? "3", 10);
const here = path.dirname(fileURLToPath(import.meta.url));
const app = path.join(here, "apps", `app-${N}`);
const OJ_BIN = process.env.OJ_BIN ?? "oj";
const ESDEV_BIN =
  process.env.ESDEV_BIN ?? path.join(here, "..", "..", "target", "release", "esdev");

const TOOLS = {
  vite: {
    port: 5210,
    host: "127.0.0.1",
    spawn: () =>
      spawn(
        process.execPath,
        [path.join(app, "node_modules", "vite", "bin", "vite.js"), "--port", String(5210), "--strictPort", "--host", "127.0.0.1"],
        { cwd: app, stdio: "ignore" }
      ),
    clearCache: () => fs.rmSync(path.join(app, "node_modules", ".vite"), { recursive: true, force: true }),
  },
  oj: {
    port: 5211,
    host: "127.0.0.1",
    spawn: () => spawn(OJ_BIN, ["dev", app, "--port", String(5211), "--bundle"], { stdio: "ignore" }),
    clearCache: () => fs.rmSync(path.join(app, ".oj-cache"), { recursive: true, force: true }),
  },
  esdev: {
    port: 5212,
    host: "127.0.0.1",
    spawn: () => spawn(ESDEV_BIN, ["start", `--port=${5212}`], { cwd: app, stdio: "ignore" }),
    clearCache: () => fs.rmSync(path.join(app, "dist"), { recursive: true, force: true }),
  },
  bun: {
    // `bun ./index.html` serves + hot-reloads the frontend with no config.
    // The port comes from PORT env, and it binds IPv6 loopback only, hence
    // host localhost. Dev serving writes nothing into the app dir, so there
    // is no cache to clear — each boot re-bundles from source.
    port: 5213,
    host: "localhost",
    spawn: () =>
      spawn("bun", ["./index.html"], {
        cwd: app,
        stdio: "ignore",
        env: { ...process.env, PORT: String(5213) },
      }),
    clearCache: () => {},
  },
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function chromePath() {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  for (const p of ["/usr/bin/chromium", "/usr/bin/google-chrome", "/usr/bin/chromium-browser"]) {
    if (fs.existsSync(p)) return p;
  }
  throw new Error("no Chrome/Chromium found — set CHROME_PATH");
}

async function waitForServer(host, port, timeoutMs = 120000) {
  const t0 = Date.now();
  const url = `http://${host}:${port}/`;
  for (;;) {
    try {
      const res = await fetch(url);
      if (res.ok) return;
      await res.arrayBuffer().catch(() => {});
    } catch {}
    if (Date.now() - t0 > timeoutMs) throw new Error(`server on ${host}:${port} did not come up`);
    await sleep(50);
  }
}

async function renderOnce(browser, host, port) {
  const page = await browser.newPage();
  const t0 = Date.now();
  await page.goto(`http://${host}:${port}/`, { timeout: 180000 });
  await page.waitForSelector("[data-done]", { timeout: 180000 });
  const ms = Date.now() - t0;
  await page.close();
  return ms;
}

function peakMb(pid) {
  try {
    const status = fs.readFileSync(`/proc/${pid}/status`, "utf8");
    const m = status.match(/^VmHWM:\s+(\d+)\s+kB/m);
    if (m) return Math.round(parseInt(m[1], 10) / 1024);
  } catch {}
  try {
    return Math.round(parseInt(execSync(`ps -o rss= -p ${pid}`).toString().trim(), 10) / 1024);
  } catch {
    return null;
  }
}

function toolVersions() {
  const out = {};
  try {
    out.vite = JSON.parse(fs.readFileSync(path.join(app, "node_modules", "vite", "package.json"), "utf8")).version;
  } catch { out.vite = null; }
  try {
    out.oj = execFileSync(OJ_BIN, ["--version"], { stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
  } catch { out.oj = null; }
  try {
    out.bun = execFileSync("bun", ["--version"], { stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
  } catch { out.bun = null; }
  try {
    out.esdev = execFileSync(ESDEV_BIN, ["--version"], { stdio: ["ignore", "pipe", "ignore"] }).toString().trim().split("\n")[0];
  } catch { out.esdev = null; }
  return out;
}

async function bench(tool, browser) {
  const { host, port, spawn: spawnTool, clearCache } = TOOLS[tool];
  const result = { tool, cold: [], warm: [], peakMb: null };
  for (let i = 0; i < ITERS; i++) {
    clearCache();
    let t0 = Date.now();
    let proc = spawnTool();
    await waitForServer(host, port);
    const ready = Date.now() - t0;
    const render = await renderOnce(browser, host, port);
    result.cold.push(ready + render);
    if (i === 0) result.peakMb = peakMb(proc.pid);
    proc.kill("SIGKILL");
    await sleep(700);

    t0 = Date.now();
    proc = spawnTool();
    await waitForServer(host, port);
    const wReady = Date.now() - t0;
    const wRender = await renderOnce(browser, host, port);
    result.warm.push(wReady + wRender);
    proc.kill("SIGKILL");
    await sleep(700);
  }
  return result;
}

const min = (xs) => Math.min(...xs);

async function main() {
  if (!fs.existsSync(path.join(app, "src"))) {
    console.error(`fixture missing: run \`node gen.mjs ${N}\` first`);
    process.exit(2);
  }
  if (!fs.existsSync(path.join(app, "node_modules"))) {
    console.error("installing fixture deps (react, vite) — one-time, needs network…");
    execSync("npm install --no-audit --no-fund", { cwd: app, stdio: ["ignore", "ignore", "inherit"] });
  }
  if (!fs.existsSync(ESDEV_BIN)) {
    console.error(`esdev not found at ${ESDEV_BIN} — build it: cargo build --release -p es-runtime-dev-cli`);
    process.exit(2);
  }

  const browser = await puppeteer.launch({
    executablePath: chromePath(),
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
  });
  const rows = [];
  try {
    for (const tool of Object.keys(TOOLS)) {
      console.error(`dev-server: ${tool} on ${N} components…`);
      rows.push(await bench(tool, browser));
    }
  } finally {
    await browser.close();
  }

  const versions = toolVersions();
  if (process.env.BENCH_JSON) {
    const dev_server = {};
    for (const r of rows) {
      dev_server[r.tool] = { cold_ms: min(r.cold), warm_ms: min(r.warm), peak_mb: r.peakMb };
    }
    console.log(JSON.stringify({
      dev_server,
      dev_server_method: {
        fixture: `fanout-10 React tree, ${N} components`,
        legs: { vite: "vite dev (default)", oj: "oj dev --bundle", esdev: "esdev start", bun: "bun ./index.html" },
        iters: ITERS,
        aggregate: "min",
        versions,
      },
    }, null, 2));
    return;
  }

  console.log(`\n${N} components (fanout-10 tree), ${ITERS} cold+warm sessions each — min, spawn-to-painted`);
  console.log("tool  | cold start | warm start | peak RSS");
  console.log("------|------------|------------|----------");
  for (const r of rows) {
    console.log(
      `${r.tool.padEnd(5)} | ${String(min(r.cold) + "ms").padEnd(10)} | ${String(min(r.warm) + "ms").padEnd(10)} | ${r.peakMb}MB`
    );
  }
  console.error(`versions: ${JSON.stringify(versions)}`);
}

await main();
