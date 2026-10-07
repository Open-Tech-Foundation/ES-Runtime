// Node.js — sharp (libvips). Node has no image API of its own, and sharp is
// what a Node application uses.
import process from "node:process";
import { readFile } from "node:fs/promises";
import sharp from "sharp";
import { JOBS, WORKLOADS } from "./workload.mjs";

const [workload, dir] = process.argv.slice(2);
const w = WORKLOADS[workload];
const bytes = await readFile(`${dir}/${w.input}`);
const outputs = await Promise.all(
  Array.from({ length: JOBS }, () => sharp(bytes).resize(w.width)[w.format]({ quality: 80 }).toBuffer()),
);
const m = await sharp(outputs[0]).metadata();
// The checksum, then the first output's size: encoders are only comparable
// at similar sizes, so the size is published beside the time.
console.log(`${outputs.length * m.width * m.height} ${outputs[0].length}`);
