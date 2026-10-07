// Bun — `Bun.Image`.
import { JOBS, WORKLOADS } from "./workload.mjs";

const [workload, dir] = process.argv.slice(2);
const w = WORKLOADS[workload];
const bytes = await Bun.file(`${dir}/${w.input}`).bytes();
const outputs = await Promise.all(
  Array.from({ length: JOBS }, () => new Bun.Image(bytes).resize(w.width)[w.format]({ quality: 80 }).bytes()),
);
const m = await new Bun.Image(outputs[0]).metadata();
console.log(outputs.length * m.width * m.height);
