// esrun — runtime:images.
import { Image } from "runtime:images";
import { file, write } from "runtime:fs";
import { args } from "runtime:process";
import { probe } from "./cases.mjs";

const png = (image) => image.png().bytes();
await probe(args[0], args[1], args[2], {
  read: (path) => file(path).bytes(),
  save: (path, bytes) => write(path, bytes),
  decodeResize: (b, w) => png(new Image(b).resize(w)),
  encode: (b, format) => new Image(b)[format]().bytes(),
  fit: (b, fit) => png(new Image(b).resize(100, 100, { fit, background: "#00000000" })),
  crop: (b) => png(new Image(b).crop({ left: 10, top: 10, width: 20, height: 20 })),
  rotate: (b) => png(new Image(b).rotate(90)),
  // Any other angle is a RangeError: the probe reports it, features.py judges it.
  rotateFree: (b) => png(new Image(b).rotate(45)),
  blur: (b) => png(new Image(b).blur(2)),
  composite: (b) => {
    if (typeof Image.prototype.composite !== "function") throw Object.assign(new Error("no composite"), { code: "UNSUPPORTED_BY_PROBE" });
    return png(new Image(b).composite(new Image(b)));
  },
  text: () => {
    throw Object.assign(new Error("no text overlay"), { code: "UNSUPPORTED_BY_PROBE" });
  },
  // GIF output is a single frame even for an animated input.
  animatedGif: (b) => new Image(b).resize(16).gif().bytes(),
  // Baseline JPEG only: the option is a TypeError.
  progressiveJpeg: (b) => new Image(b).jpeg({ progressive: true }).bytes(),
  reencode: (b) => png(new Image(b)),
  frames: async (b) => (await new Image(b).metadata()).animation?.frames ?? null,
});
