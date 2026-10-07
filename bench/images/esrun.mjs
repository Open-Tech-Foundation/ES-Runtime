// esrun — `runtime:images`.
import { Image } from "runtime:images";
import { file } from "runtime:fs";
import { args } from "runtime:process";
import { JOBS, WORKLOADS } from "./workload.mjs";

const [workload, dir] = args;
const w = WORKLOADS[workload];
const bytes = await file(`${dir}/${w.input}`).bytes();
const outputs = await Promise.all(
  Array.from({ length: JOBS }, () => new Image(bytes).resize(w.width)[w.format]({ quality: 80 }).bytes()),
);
const m = await new Image(outputs[0]).metadata();
// The checksum, then the first output's size: encoders are only comparable
// at similar sizes, so the size is published beside the time.
console.log(`${outputs.length * m.width * m.height} ${outputs[0].length}`);
