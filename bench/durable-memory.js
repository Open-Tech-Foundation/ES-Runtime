// Durable workers: what memory costs, and that it stops growing.
//
// Run from the repository root:
//
//     esdev bench/durable-memory.js
//
// It opens a stream of new workers — one write each — under several `maxLive`
// settings, and reports the process's resident memory next to the JS heap.
// Two questions, because they are what the documentation claims:
//
//   * **Does memory stop growing under churn?** It should plateau once
//     `maxLive` workers are open: everything past that is evicted.
//   * **What does one live worker cost?** Resident memory grows with `maxLive`
//     while the heap does not: the cost is native, per open database file (the
//     SQL engine gives each one a buffer arena of its own).
//
// Each setting runs in a child process, so one setting's memory cannot colour
// the next one's numbers.

import { Command } from "runtime:system";
import { env, unmask } from "runtime:process";

const ESDEV = unmask(env.ESDEV ?? "esdev");
const WORKERS = 2000;
const SETTINGS = [16, 64, 128, 256];

const child = (maxLive) => `
import { DurableWorker, configure, shutdown } from "runtime:workers";
import { memoryUsage } from "runtime:process";
import { metrics } from "runtime:diagnostics";
import { makeTempDir, remove } from "runtime:fs";
const dir = await makeTempDir({ prefix: "durable-memory-" });
configure({ dir, maxLive: ${maxLive} });
class C extends DurableWorker {
  add(n) { this.state.set("cart", [{ id: "x", qty: n }]); return n; }
}
const mb = (b) => Math.round(b / 1048576);
const samples = [];
for (let i = 1; i <= ${WORKERS}; i++) {
  await C.get("c" + i).add(i);
  if (i % 500 === 0) samples.push(mb(metrics().process.rss));
}
const heap = mb(memoryUsage().heapUsed);
await shutdown();
await remove(dir, { recursive: true });
console.log(JSON.stringify({ samples, heap }));
`;

console.log(`\n## ${WORKERS} new workers, one write each\n`);
console.log("| `maxLive` | Resident memory at 500 / 1000 / 1500 / 2000 workers | JS heap |");
console.log("| --- | --- | --- |");
const rows = [];
for (const maxLive of SETTINGS) {
  const { success, stdout, stderr } = await new Command(ESDEV, {
    args: ["-e=" + child(maxLive)],
    inheritEnv: true,
  }).output();
  if (!success) throw new Error(new TextDecoder().decode(stderr));
  const { samples, heap } = JSON.parse(new TextDecoder().decode(stdout).trim().split("\n").pop());
  rows.push({ maxLive, final: samples.at(-1) });
  console.log(`| ${maxLive} | ${samples.map((m) => `${m} MB`).join(" / ")} | ${heap} MB |`);
}

// The slope between the smallest and the largest setting: what one more open
// worker adds.
const [lo, hi] = [rows[0], rows.at(-1)];
const perWorker = (hi.final - lo.final) / (hi.maxLive - lo.maxLive);
console.log(`\nEach open worker adds about ${perWorker.toFixed(1)} MB of resident memory.`);
