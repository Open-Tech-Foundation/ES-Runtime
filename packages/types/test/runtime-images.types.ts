// A type test for `runtime:images`. Compiled by `tsc -p .`, never run.

import { file } from "runtime:fs";
import { Image, type ImageMetadata, type ImageStats } from "runtime:images";

const image = new Image(new Uint8Array(0), { maxPixels: 1_000_000, autoOrient: false, page: 1 });
const fromFile = new Image(file("photo.jpg"));
const fromBlob = new Image(new Blob([]));

const chain: Image = image
  .resize(400, null, {
    fit: "cover",
    filter: "mitchell",
    background: "#fff",
    position: "north",
    withoutReduction: true,
  })
  .crop({ width: 10, height: 10 })
  .rotate(90)
  .modulate({ hue: 30, lightness: 10 })
  .negate({ alpha: false })
  .gamma(2.2)
  .linear(1.2, 10)
  .linear([1, 1, 1], [0, 10, 10])
  .threshold(200, { greyscale: false })
  .tint("#ff0000")
  .recomb([
    [1, 0, 0],
    [0, 1, 0],
    [0, 0, 1],
  ])
  .ensureAlpha(0.5)
  .removeAlpha()
  .greyscale()
  .flatten({ background: { r: 255, g: 255, b: 255 } })
  .extractChannel("red")
  .webp({ quality: 80, lossless: false });

async function use() {
  const meta: ImageMetadata = await chain.metadata();
  const pages: number = meta.pages;
  const density: number | undefined = meta.density;
  const exif: Uint8Array | undefined = meta.exif;
  const stats: ImageStats = await chain.stats();
  const opaque: boolean = stats.isOpaque && stats.channels[0].stdev >= 0;
  const jxl: Blob = await image
    .extend({ top: 2, extendWith: "mirror" })
    .extend(4)
    .trim({ threshold: 20, lineArt: true })
    .composite([
      { input: fromBlob, gravity: "southeast", blend: "multiply" },
      { input: new Uint8Array(0), left: 2, top: 2, tile: true },
      { input: file("logo.png") },
    ])
    .keepMetadata()
    .withXmp("<x/>")
    .withMetadata({ density: 300 })
    .jxl({ lossless: true })
    .blob();
  await image.jpeg({ progressive: true, chromaSubsampling: "4:4:4" }).bytes();
  const pixels: Uint8Array = await new Image(new Uint8Array(4), {
    raw: { width: 2, height: 2, channels: 1 },
  })
    .convolve({ width: 3, height: 1, kernel: [1, 2, 1] })
    .raw({ depth: "ushort" })
    .bytes();
  await new Image({ create: { width: 64, height: 64, channels: 4, background: "#fff0" } })
    .composite([{ input: pixels, raw: { width: 2, height: 2, channels: 1, depth: "ushort" } }])
    .png()
    .bytes();
  await image.png({ palette: true, colors: 64, dither: false, compressionLevel: 9 }).bytes();
  await image.webp({ effort: 6, alphaQuality: 80, nearLossless: true }).bytes();
  await image.avif({ chromaSubsampling: "4:4:4", effort: 2 }).bytes();
  await image.gif({ colours: 16, dither: 0.5 }).keepExif().keepXmp().bytes();
  const bytes: Uint8Array = await chain.bytes();
  const blob: Blob = await fromBlob.avif().blob();
  const written: number = await fromFile.png().write(file("out.png"));
  return [meta.animation?.frames, pages, density, exif, opaque, jxl, bytes, blob, written];
}

// @ts-expect-error — a path string is refused: pass file(path).
new Image("photo.jpg");
// @ts-expect-error — not a fit.
image.resize(10, 10, { fit: "stretch" });
// @ts-expect-error — not a position.
image.resize(10, 10, { position: "attention" });
// @ts-expect-error — not a blend mode.
image.composite([{ input: fromBlob, blend: "burn" }]);
// @ts-expect-error — create takes 3 or 4 channels.
new Image({ create: { width: 1, height: 1, channels: 2, background: "#000" } });
// @ts-expect-error — not a depth Pixels has.
image.raw({ depth: "double" });
// @ts-expect-error — not a chroma subsampling.
image.jpeg({ chromaSubsampling: "4:2:2" });
// @ts-expect-error — write takes a file().
image.write("out.png");

void use;
