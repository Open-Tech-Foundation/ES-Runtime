// Node.js — sharp, which is what a Node application uses: Node has no image API.
import process from "node:process";
import { readFile, writeFile } from "node:fs/promises";
import sharp from "sharp";
import { probe } from "./cases.mjs";

const png = (s) => s.png().toBuffer();
await probe(process.argv[2], process.argv[3], process.argv[4], {
  read: (path) => readFile(path),
  save: (path, bytes) => writeFile(path, bytes),
  decodeResize: (b, w) => png(sharp(b).resize(w)),
  // sharp names the codec `heif`, not `heic` — and 0.35+ needs an explicit
  // compression: HEVC keeps the `heic` brand (AV1 would write AVIF).
  encode: (b, format) => (format === "heic"
    ? sharp(b).heif({ compression: "hevc" })
    : sharp(b)[format]()
  ).toBuffer(),
  fit: (b, fit) => png(sharp(b).resize(100, 100, { fit, background: { r: 0, g: 0, b: 0, alpha: 0 } })),
  crop: (b) => png(sharp(b).extract({ left: 10, top: 10, width: 20, height: 20 })),
  rotate: (b) => png(sharp(b).rotate(90)),
  rotateFree: (b) => png(sharp(b).rotate(45)),
  blur: (b) => png(sharp(b).blur(2)),
  composite: (b) => png(sharp(b).composite([{ input: b }])),
  text: (b) => png(sharp(b).composite([{
    input: Buffer.from(
      '<svg width="64" height="48"><rect width="64" height="48" fill="white"/>' +
      '<text x="4" y="38" font-size="34" font-family="sans-serif" fill="black">X</text></svg>',
    ),
  }])),
  animatedGif: (b) => sharp(b, { animated: true }).resize(16).gif().toBuffer(),
  progressiveJpeg: (b) => sharp(b).jpeg({ progressive: true }).toBuffer(),
  reencode: (b) => png(sharp(b)),
  frames: async (b) => (await sharp(b).metadata()).pages ?? null,
});
