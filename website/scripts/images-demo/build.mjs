// Builds the homepage's image demo: every picture it shows and every number
// beside one is made here, by runtime:images, from one source photo.
//
//   ../target/debug/esdev scripts/images-demo/build.mjs     (from website/)
//
// Run it with this checkout's esdev (`cargo build -p es-runtime-dev-cli`), not
// an installed release, so the demo shows the runtime it is published with.
// It writes public/img/demo/* and components/imagesDemoData.js; commit both.
//
// Source: "Cosmic Cliffs" in the Carina Nebula, James Webb Space Telescope
// NIRCam, images.nasa.gov `carina_nebula` (large rendition). Credit: NASA, ESA,
// CSA, STScI. Licensed CC BY 4.0 (esawebb.org/copyright); the page shows the
// credit and says what was changed.

import { Image } from "runtime:images";
import { file, write } from "runtime:fs";

// runtime:fs resolves a relative path against this module's directory.
const OUT = "../../public/img/demo";
const DATA = "../../components/imagesDemoData.js";
const SOURCE = "cosmic-cliffs.jpg";

const source = await file(SOURCE).bytes();
const photo = new Image(source);

async function save(name, image) {
  const bytes = await image.bytes();
  await write(`${OUT}/${name}`, bytes);
  const meta = await new Image(bytes).metadata();
  return {
    src: `/img/demo/${name}`,
    format: meta.format,
    width: meta.width,
    height: meta.height,
    bytes: bytes.length,
    frames: meta.animation?.frames ?? 1,
  };
}

// The site's mark, drawn as RGBA at `size` pixels with 4×4 supersampling:
// public/img/otf-logo.svg is a translucent dark disc under three orange bars.
function logo(size) {
  const scale = size / 640;
  const disc = { cx: 320.75, cy: 320.61, r: 295.8 };
  const bars = [
    [317.22, 176.78, 168.5, 32.4],
    [368.74, 297.93, 56.5, 32.5],
    [312.29, 333.7, 32.4, 189.5],
  ];
  const pixels = new Uint8Array(size * size * 4);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let discHits = 0;
      let barHits = 0;
      for (let sy = 0; sy < 4; sy++) {
        for (let sx = 0; sx < 4; sx++) {
          const px = (x + (sx + 0.5) / 4) / scale;
          const py = (y + (sy + 0.5) / 4) / scale;
          if ((px - disc.cx) ** 2 + (py - disc.cy) ** 2 <= disc.r ** 2) discHits++;
          if (bars.some(([cx, cy, hw, hh]) => Math.abs(px - cx) <= hw && Math.abs(py - cy) <= hh)) barHits++;
        }
      }
      // The bars sit on the disc; where both cover, the bar's orange wins.
      const bar = barHits / 16;
      const disk = (discHits / 16) * 0.85;
      const alpha = bar + disk * (1 - bar);
      const mix = alpha === 0 ? 0 : bar / alpha;
      const i = (y * size + x) * 4;
      pixels[i] = Math.round(255 * mix + 17 * (1 - mix));
      pixels[i + 1] = Math.round(133 * mix + 17 * (1 - mix));
      pixels[i + 2] = Math.round(27 * mix + 17 * (1 - mix));
      pixels[i + 3] = Math.round(alpha * 255);
    }
  }
  return new Image(pixels, { raw: { width: size, height: size, channels: 4 } });
}

// GIF89a, from one single-frame GIF per frame: each frame's global colour
// table becomes its local one. runtime:images writes an animation only from
// an animated source, and the demo needs one to start from.
function animatedGif(stills, width, height, delayMs) {
  const parts = [];
  const u16 = (n) => [n & 0xff, n >> 8];
  parts.push(new TextEncoder().encode("GIF89a"));
  parts.push(new Uint8Array([...u16(width), ...u16(height), 0x00, 0, 0]));
  // NETSCAPE2.0: loop forever.
  parts.push(new Uint8Array([0x21, 0xff, 0x0b, ...new TextEncoder().encode("NETSCAPE2.0"), 0x03, 0x01, 0, 0, 0x00]));
  for (const gif of stills) {
    const packed = gif[10];
    if (!(packed & 0x80)) throw new Error("expected a global colour table");
    const tableBits = packed & 0x07;
    const table = gif.subarray(13, 13 + 3 * (2 << tableBits));
    let at = 13 + table.length;
    // Skip the still's own extensions; the frame gets its delay below.
    while (gif[at] === 0x21) {
      at += 2;
      while (gif[at] !== 0) at += gif[at] + 1;
      at += 1;
    }
    if (gif[at] !== 0x2c) throw new Error("expected an image descriptor");
    const descriptor = gif.slice(at, at + 10);
    if (descriptor[9] & 0x80) throw new Error("expected no local colour table");
    descriptor[9] = (descriptor[9] & 0x40) | 0x80 | tableBits;
    at += 10;
    const start = at;
    at += 1; // LZW minimum code size
    while (gif[at] !== 0) at += gif[at] + 1;
    at += 1;
    const centiseconds = Math.round(delayMs / 10);
    parts.push(new Uint8Array([0x21, 0xf9, 0x04, 0x04, ...u16(centiseconds), 0, 0]));
    parts.push(descriptor, table, gif.subarray(start, at));
  }
  parts.push(new Uint8Array([0x3b]));
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}

const meta = await photo.metadata();

// 1. One thumbnail, four formats.
const thumb = photo.resize(480, 480, { fit: "cover" });
const formats = {};
for (const [format, ext, options] of [
  ["jpeg", "jpg", {}],
  ["webp", "webp", {}],
  ["avif", "avif", { quality: 50 }],
  ["jxl", "jxl", {}],
]) {
  formats[format] = await save(`thumb.${ext}`, thumb[format](options));
}

// 2. A watermark in the corner, inset by a transparent margin.
const mark = logo(72).extend({ right: 16, bottom: 16, background: "#0000" });
const watermark = await save(
  "watermark.webp",
  thumb.composite([{ input: mark, gravity: "southeast" }]).webp(),
);

// 3. Any angle, onto a transparent background.
const rotated = await save(
  "rotated.webp",
  photo.resize(360, 360, { fit: "cover", position: "west" }).rotate(12, { background: "#0000" }).webp(),
);

// 4. An animation: a slow pan along the cliffs and back, made here as the
//    input, then resized frame by frame by runtime:images. Out and back, so
//    the loop has no jump.
const STEPS = 8;
const SPAN = 400;
const stills = [];
for (let i = 0; i < 2 * STEPS; i++) {
  const step = i <= STEPS ? i : 2 * STEPS - i;
  const left = 380 + Math.round((step * SPAN) / STEPS);
  stills.push(await photo.crop({ left, top: 260, width: 960, height: 540 }).resize(480, 270).gif().bytes());
}
const panGif = animatedGif(stills, 480, 270, 140);
const pan = await new Image(panGif, { animated: true }).metadata();
const animated = await save("animated.webp", new Image(panGif, { animated: true }).resize(320).webp({ quality: 60 }));

const data = {
  source: { name: "cosmic-cliffs.jpg", width: meta.width, height: meta.height, bytes: source.length },
  formats,
  watermark,
  rotated,
  animated: {
    input: { format: "gif", width: pan.width, height: pan.height, frames: pan.frames, bytes: panGif.length },
    output: animated,
  },
};

await write(
  DATA,
  `// Generated by scripts/images-demo/build.mjs. Do not edit: rerun it.\nexport default ${JSON.stringify(data, null, 2)};\n`,
);
console.log(JSON.stringify(data, null, 2));
