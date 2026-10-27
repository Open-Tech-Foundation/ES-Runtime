// esrun — `runtime:pdf`.
import { PDF } from "runtime:pdf";
import { file } from "runtime:fs";
import { args } from "runtime:process";
import { JOBS, WORKLOADS, count } from "./workload.mjs";

const [workload, dir] = args;
const w = WORKLOADS[workload];
const bytes = await file(`${dir}/${w.input}`).bytes();

const run = {
  async text() {
    const doc = await PDF.open(bytes);
    return count(await doc.text(), "invoice");
  },
  async merge() {
    const doc = await PDF.open(bytes);
    return (await PDF.open(await doc.edit().append(doc).bytes())).pageCount;
  },
  async split() {
    const doc = await PDF.open(bytes);
    const first = Array.from({ length: w.pages }, (_, i) => i);
    return (await PDF.open(await doc.edit().select(first).bytes())).pageCount;
  },
}[workload];

const results = await Promise.all(Array.from({ length: JOBS }, run));
console.log(results.reduce((a, b) => a + b, 0));
