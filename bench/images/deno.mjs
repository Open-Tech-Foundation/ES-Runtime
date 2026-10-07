// Deno — its own image path: `createImageBitmap` decodes and resizes, an
// `OffscreenCanvas` "bitmaprenderer" context holds the result, and
// `convertToBlob` encodes it. That encoder writes PNG and JPEG only; asked for
// another type it returns PNG, as the HTML spec allows, so WebP and AVIF are n/a.
import { JOBS, WORKLOADS } from "./workload.mjs";

const ENCODES = new Set(["jpeg", "png"]);
const [workload, dir] = Deno.args;
const w = WORKLOADS[workload];
if (!ENCODES.has(w.format)) {
  console.log("n/a");
  Deno.exit(0);
}
const bytes = await Deno.readFile(`${dir}/${w.input}`);

async function thumbnail() {
  const bitmap = await createImageBitmap(new Blob([bytes]), { resizeWidth: w.width, resizeQuality: "high" });
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  canvas.getContext("bitmaprenderer").transferFromImageBitmap(bitmap);
  const blob = await canvas.convertToBlob({ type: `image/${w.format}`, quality: 0.8 });
  return new Uint8Array(await blob.arrayBuffer());
}

const outputs = await Promise.all(Array.from({ length: JOBS }, thumbnail));
const m = await createImageBitmap(new Blob([outputs[0]]));
// The checksum, then the first output's size: encoders are only comparable
// at similar sizes, so the size is published beside the time.
console.log(`${outputs.length * m.width * m.height} ${outputs[0].length}`);
