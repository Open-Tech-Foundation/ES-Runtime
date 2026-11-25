// runtime:images churn: decode, resize and encode in every output format the
// module writes, several at once; an animation resized frame by frame and
// written in each animated format; and runs aborted part way. The work runs off
// the event loop on a shared pool, so this watches that pool, the buffers it
// hands back, and the cancel flags a signal files with the host.
import { Image } from "runtime:images";
import { file } from "runtime:fs";

const source = await new Image({ create: { width: 640, height: 480, channels: 4, background: "#c82a5a" } })
  .png()
  .bytes();
const animation = await file("animated.gif").bytes();
const FORMATS = ["png", "jpeg", "webp", "gif", "tiff", "avif", "jxl"];
const ANIMATED = ["gif", "webp", "jxl"];
let done = 0;

async function stills(offset) {
  for (let i = offset; ; i++) {
    const format = FORMATS[i % FORMATS.length];
    const out = await new Image(source).resize(160)[format]().bytes();
    if (out.byteLength === 0) throw new Error(`${format} produced no bytes`);
    const { width } = await new Image(out).metadata();
    if (width !== 160) throw new Error(`${format} came back ${width} wide`);
    done++;
  }
}

async function animations() {
  for (let i = 0; ; i++) {
    const format = ANIMATED[i % ANIMATED.length];
    const out = await new Image(animation, { animated: true }).resize(16).rotate(30)[format]({ loop: 2 }).bytes();
    const meta = await new Image(out).metadata();
    if (meta.animation?.frames !== 2 || meta.animation.loop !== 2) {
      throw new Error(`${format} animation came back as ${JSON.stringify(meta.animation)}`);
    }
    done++;
  }
}

// A run long enough to be stopped, stopped: half by a signal, half by the
// chain's deadline. Each must reject as it should, and leave nothing behind.
const heavy = new Image({ create: { width: 2000, height: 2000, channels: 3, background: "#123456" } }).resize(1999).raw();
async function aborted() {
  for (let i = 0; ; i++) {
    try {
      if (i % 2 === 0) {
        const controller = new AbortController();
        setTimeout(() => controller.abort(new Error("stop")), 5);
        await heavy.bytes({ signal: controller.signal });
      } else {
        await heavy.timeout({ seconds: 0.005 }).bytes();
      }
    } catch (e) {
      if (e.message !== "stop" && e.name !== "TimeoutError") throw e;
    }
    done++;
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([stills(0), stills(1), stills(2), animations(), aborted()]);
