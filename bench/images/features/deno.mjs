// Deno — its built-in path: createImageBitmap decodes, resizes and crops; an
// OffscreenCanvas "bitmaprenderer" context and convertToBlob encode.
import { probe, unsupported } from "./cases.mjs";

async function encode(bitmap, type = "image/png") {
  const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
  canvas.getContext("bitmaprenderer").transferFromImageBitmap(bitmap);
  return new Uint8Array(await (await canvas.convertToBlob({ type })).arrayBuffer());
}
const bitmap = (b, ...rest) => createImageBitmap(new Blob([b]), ...rest);
await probe(Deno.args[0], Deno.args[1], Deno.args[2], {
  read: (path) => Deno.readFile(path),
  save: (path, bytes) => Deno.writeFile(path, bytes),
  decodeResize: async (b, w) => encode(await bitmap(b, { resizeWidth: w })),
  encode: async (b, format) => encode(await bitmap(b), `image/${format}`),
  fit: () => unsupported(),
  crop: async (b) => encode(await bitmap(b, 10, 10, 20, 20)),
  rotate: () => unsupported(),
  rotateFree: () => unsupported(),
  blur: () => unsupported(),
  composite: () => unsupported(),
  text: () => unsupported(),
  animatedGif: () => unsupported(),
  progressiveJpeg: () => unsupported(),
  extend: () => unsupported(),
  trim: () => unsupported(),
  convolve: () => unsupported(),
  blend: () => unsupported(),
  // ImageData in, through createImageBitmap — the one way in for raw pixels.
  rawIn: async (b, { width, height }) => {
    const rgba = new Uint8ClampedArray(width * height * 4);
    for (let i = 0; i < width * height; i++) rgba.set([b[i * 3], b[i * 3 + 1], b[i * 3 + 2], 255], i * 4);
    return encode(await createImageBitmap(new ImageData(rgba, width, height)));
  },
  // No 2d context, so no getImageData.
  rawOut: () => unsupported(),
  create: () => unsupported(),
  keepExif: async (b) => encode(await bitmap(b), "image/jpeg"),
  blueMean: () => unsupported(),
  reencode: async (b) => encode(await bitmap(b)),
  frames: () => null,
});
