// Bun — `Bun.Image`.
import { JOBS, WORKLOADS } from "./workload.mjs";

const [workload, dir] = process.argv.slice(2);
const w = WORKLOADS[workload];
const bytes = await Bun.file(`${dir}/${w.input}`).bytes();
let outputs;
try {
  outputs = await Promise.all(
    Array.from({ length: JOBS }, () => new Bun.Image(bytes).resize(w.width)[w.format]({ quality: 80 }).bytes()),
  );
} catch (e) {
  // A format this platform's Bun cannot encode (AVIF on Linux) is n/a, not a zero.
  if (e.code === "ERR_IMAGE_FORMAT_UNSUPPORTED") {
    console.log("n/a");
    process.exit(0);
  }
  throw e;
}
const m = await new Bun.Image(outputs[0]).metadata();
// The checksum, then the first output's size: encoders are only comparable
// at similar sizes, so the size is published beside the time.
console.log(`${outputs.length * m.width * m.height} ${outputs[0].length}`);
