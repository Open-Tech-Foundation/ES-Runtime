// Bun — Bun.Image.
import { probe, unsupported } from "./cases.mjs";

const png = (image) => image.png().bytes();
const has = (name) => typeof Bun.Image.prototype[name] === "function";
await probe(process.argv[2], process.argv[3], process.argv[4], {
  read: (path) => Bun.file(path).bytes(),
  save: (path, bytes) => Bun.write(path, bytes),
  decodeResize: (b, w) => png(new Bun.Image(b).resize(w)),
  encode: (b, format) => (has(format) ? new Bun.Image(b)[format]().bytes() : unsupported()),
  fit: (b, fit) => png(new Bun.Image(b).resize(100, 100, { fit })),
  crop: (b) => (has("crop") ? png(new Bun.Image(b).crop({ left: 10, top: 10, width: 20, height: 20 })) : unsupported()),
  rotate: (b) => png(new Bun.Image(b).rotate(90)),
  blur: (b) => (has("blur") ? png(new Bun.Image(b).blur(2)) : unsupported()),
  composite: (b) => (has("composite") ? png(new Bun.Image(b).composite(new Bun.Image(b))) : unsupported()),
  reencode: (b) => png(new Bun.Image(b)),
  frames: async (b) => {
    const m = await new Bun.Image(b).metadata();
    return m.frames ?? m.pages ?? m.animation?.frames ?? null;
  },
});
