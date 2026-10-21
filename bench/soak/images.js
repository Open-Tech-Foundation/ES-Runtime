// runtime:images churn: decode, resize and encode in every output format the
// module writes, several at once. The work runs off the event loop on a shared
// pool, so this watches that pool and the buffers it hands back.
import { Image } from "runtime:images";

const source = await new Image({ create: { width: 640, height: 480, channels: 4, background: "#c82a5a" } })
  .png()
  .bytes();
const FORMATS = ["png", "jpeg", "webp", "gif", "tiff", "avif"];
let done = 0;

async function loop(offset) {
  for (let i = offset; ; i++) {
    const format = FORMATS[i % FORMATS.length];
    const out = await new Image(source).resize(160)[format]().bytes();
    if (out.byteLength === 0) throw new Error(`${format} produced no bytes`);
    const { width } = await new Image(out).metadata();
    if (width !== 160) throw new Error(`${format} came back ${width} wide`);
    done++;
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([loop(0), loop(1), loop(2), loop(3)]);
