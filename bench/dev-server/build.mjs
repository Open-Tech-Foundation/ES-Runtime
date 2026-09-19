// Production-build benchmark: vite build vs oj build vs esdev build vs
// bun build on the generated fixture (bench/dev-server/apps/app-<N>).
//
// Each leg runs the tool's default production build into its own outdir,
// with minification on everywhere it is a flag (vite and oj minify by
// default; esdev and bun get --minify), so time and size compare like for
// like. Every rep clears the outdir first and records wall time, peak RSS
// (polled off the child while it runs — a finished build leaves no /proc
// entry for a VmHWM read, so the high-water mark is sampled, not read once)
// and output bytes. Published time per cell is the MIN over reps (repo
// convention); published memory is the peak of the fastest rep, so the row
// describes one real run. After its reps, each tool's output is served
// statically and must mount in
// a real browser ([data-done]) before its numbers publish — a build that
// does not render is a failure, not a fast time.
//
// Usage:
//   node build.mjs [N]              human table (default 10000)
//   BENCH_JSON=1 node build.mjs     machine JSON for gen-bench-data.sh
//
// Knobs: BENCH_ITERS (default 3), OJ_BIN, ESDEV_BIN.
import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
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

const OUTDIRS = {
  vite: "dist-vite",
  oj: "dist-oj",
  esdev: "dist",
  bun: "dist-bun",
};

const TOOLS = {
  vite: {
    cmd: [process.execPath, [path.join(app, "node_modules", "vite", "bin", "vite.js"), "build", "--outDir", OUTDIRS.vite]],
    opts: { cwd: app },
  },
  oj: {
    cmd: [OJ_BIN, ["build", app, "--outDir", path.join(app, OUTDIRS.oj)]],
    opts: {},
  },
  esdev: {
    cmd: [ESDEV_BIN, ["build", "--minify"]],
    opts: { cwd: app },
  },
  bun: {
    // NODE_ENV=production is explicit because bun, unlike the other three,
    // does not default it at build time — without it React resolves to its
    // development build (+1.5 MB of warnings and DevTools nags).
    cmd: ["bun", ["build", "./index.html", "--outdir", path.join(app, OUTDIRS.bun), "--minify"]],
    opts: { cwd: app, env: { ...process.env, NODE_ENV: "production" } },
  },
};

function outBytes(tool) {
  const dir = path.join(app, OUTDIRS[tool]);
  let total = 0;
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else total += fs.statSync(p).size;
    }
  };
  try {
    walk(dir);
  } catch {
    return null;
  }
  return total;
}

function readRssKb(pid) {
  try {
    const status = fs.readFileSync(`/proc/${pid}/status`, "utf8");
    const m = status.match(/^VmRSS:\s+(\d+)\s+kB/m);
    if (m) return parseInt(m[1], 10);
  } catch {}
  try {
    const out = execFileSync("ps", ["-o", "rss=", "-p", String(pid)], {
      stdio: ["ignore", "pipe", "ignore"],
    }).toString().trim();
    const kb = parseInt(out, 10);
    if (Number.isFinite(kb)) return kb;
  } catch {}
  return null;
}

function runOnce(tool) {
  const { cmd, opts } = TOOLS[tool];
  fs.rmSync(path.join(app, OUTDIRS[tool]), { recursive: true, force: true });
  const t0 = Date.now();
  return new Promise((resolve, reject) => {
    const proc = spawn(cmd[0], cmd[1], { stdio: "ignore", ...opts });
    let peakKb = null;
    const sample = () => {
      if (proc.pid === undefined) return;
      const kb = readRssKb(proc.pid);
      if (kb !== null && (peakKb === null || kb > peakKb)) peakKb = kb;
    };
    sample();
    const timer = setInterval(sample, 10);
    proc.on("error", (err) => {
      clearInterval(timer);
      reject(err);
    });
    proc.on("exit", (code) => {
      clearInterval(timer);
      sample();
      if (code !== 0) reject(new Error(`${tool} build exited ${code}`));
      else resolve({ ms: Date.now() - t0, peakKb });
    });
  });
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

function chromePath() {
  if (process.env.CHROME_PATH) return process.env.CHROME_PATH;
  for (const p of ["/usr/bin/chromium", "/usr/bin/google-chrome", "/usr/bin/chromium-browser"]) {
    if (fs.existsSync(p)) return p;
  }
  throw new Error("no Chrome/Chromium found — set CHROME_PATH");
}

const MIME = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".map": "application/json",
  ".png": "image/png",
  ".ico": "image/x-icon",
  ".txt": "text/plain",
};

function serveStatic(dir, port) {
  return new Promise((resolve) => {
    const server = http.createServer((req, res) => {
      const urlPath = new URL(req.url, "http://x").pathname;
      const file = path.normalize(path.join(dir, urlPath === "/" ? "index.html" : urlPath.slice(1)));
      if (!file.startsWith(dir)) {
        res.writeHead(403);
        res.end();
        return;
      }
      fs.readFile(file, (err, data) => {
        if (err) {
          res.writeHead(404);
          res.end();
          return;
        }
        res.writeHead(200, { "content-type": MIME[path.extname(file)] ?? "application/octet-stream" });
        res.end(data);
      });
    });
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
}

// A build that does not render is not a result: serve the final outdir and
// require the app to mount in a real browser before its numbers publish.
async function verifyBuild(tool, browser, port) {
  const dir = path.join(app, OUTDIRS[tool]);
  const server = await serveStatic(dir, port);
  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${port}/`, { timeout: 180000 });
    await page.waitForSelector("[data-done]", { timeout: 180000 });
    await page.close();
  } catch (e) {
    throw new Error(`${tool} output does not render: ${e.message.split("\n")[0]}`);
  } finally {
    server.close();
  }
}

async function main() {
  if (!fs.existsSync(path.join(app, "src"))) {
    console.error(`fixture missing: run \`node gen.mjs ${N}\` first`);
    process.exit(2);
  }
  if (!fs.existsSync(path.join(app, "node_modules"))) {
    console.error("installing fixture deps (react, vite) — one-time, needs network…");
    const { execSync } = await import("node:child_process");
    execSync("npm install --no-audit --no-fund", { cwd: app, stdio: ["ignore", "ignore", "inherit"] });
  }

  const rows = [];
  const browser = await puppeteer.launch({
    executablePath: chromePath(),
    args: ["--no-sandbox", "--disable-dev-shm-usage"],
  });
  let verifyPort = 5220;
  try {
    for (const tool of Object.keys(TOOLS)) {
      console.error(`build: ${tool} on ${N} components…`);
      const reps = [];
      for (let i = 0; i < ITERS; i++) reps.push(await runOnce(tool));
      await verifyBuild(tool, browser, verifyPort++);
      console.error(`build: ${tool} output renders`);
      // The published row describes the fastest rep: its time, and the peak
      // RSS sampled during that same run. Peak RSS is a floor contention
      // cannot inflate, so no further aggregation is needed.
      let best = reps[0];
      for (const r of reps) if (r.ms < best.ms) best = r;
      const peakMb = best.peakKb === null ? null : Math.round(best.peakKb / 1024);
      rows.push({ tool, ms: best.ms, bytes: outBytes(tool), peakMb, reps });
    }
  } finally {
    await browser.close();
  }

  const versions = toolVersions();
  if (process.env.BENCH_JSON) {
    const build_time = {};
    for (const r of rows) build_time[r.tool] = { build_ms: r.ms, out_kb: Math.round(r.bytes / 1024), peak_mb: r.peakMb };
    console.log(JSON.stringify({
      build_time,
      build_time_method: {
        fixture: `fanout-10 React tree, ${N} components`,
        legs: {
          vite: "vite build (minified by default)",
          oj: "oj build (minified by default)",
          esdev: "esdev build --minify",
          bun: "bun build --minify (NODE_ENV=production, which bun does not default)",
        },
        iters: ITERS,
        aggregate: "min",
        versions,
      },
    }, null, 2));
    return;
  }

  console.log(`\n${N} components (fanout-10 tree), production build, min of ${ITERS}`);
  console.log("tool  | build time | output size | peak RSS");
  console.log("------|------------|-------------|----------");
  for (const r of rows) {
    console.log(
      `${r.tool.padEnd(5)} | ${String((r.ms / 1000).toFixed(1) + "s").padEnd(10)} | ${(r.bytes / 1048576).toFixed(1).padEnd(7)} MB | ${r.peakMb === null ? "n/a" : r.peakMb + " MB"}`
    );
  }
  console.error(`versions: ${JSON.stringify(versions)}`);
  for (const r of rows) console.error(`${r.tool} reps: ${r.reps.map((x) => x.ms).join(",")}ms; peak: ${r.reps.map((x) => x.peakKb === null ? "n/a" : Math.round(x.peakKb / 1024) + "MB").join(",")}`);
}

await main();
