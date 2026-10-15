// A type test for `runtime:images`. Compiled by `tsc -p .`, never run.

import { file } from "runtime:fs";
import { Image, type ImageMetadata } from "runtime:images";

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
  const bytes: Uint8Array = await chain.bytes();
  const blob: Blob = await fromBlob.avif().blob();
  const written: number = await fromFile.png().write(file("out.png"));
  return [meta.animation?.frames, pages, bytes, blob, written];
}

// @ts-expect-error — a path string is refused: pass file(path).
new Image("photo.jpg");
// @ts-expect-error — not a fit.
image.resize(10, 10, { fit: "stretch" });
// @ts-expect-error — not a position.
image.resize(10, 10, { position: "attention" });
// @ts-expect-error — write takes a file().
image.write("out.png");

void use;
