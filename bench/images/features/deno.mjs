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
  blur: () => unsupported(),
  composite: () => unsupported(),
  reencode: async (b) => encode(await bitmap(b)),
  frames: () => null,
});
