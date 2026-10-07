// runtime:images — decode, transform and encode images (DECISIONS D159).
//
// Bun's `Image` chain on otf-pixels: `new Image(input)`, transforms that each
// return a new image, one output format, and an awaited terminal. Nothing runs
// until the terminal; then the whole pipeline goes to the host as a list of
// steps and runs off the event loop.
//
//   import { Image } from "runtime:images";
//   const thumb = await new Image(upload).resize(400, 400, { fit: "cover" }).webp().bytes();
//
// Input is bytes, a Blob, or a runtime:fs file() — never a path string, which
// would read a file without passing the filesystem provider. Bytes in and out
// need no capability; a file() source needs FileRead and a file() destination
// FileWrite, exactly as reading or writing that file would.

const ops = globalThis.__ops;
const FILE_BODY = globalThis.__internal.fileBody;
const BLOB_BYTES = globalThis.__internal.bytes;

const FITS = new Set(["fill", "inside", "outside", "cover", "contain"]);
const FILTERS = new Set([
  "nearest",
  "box",
  "bilinear",
  "linear",
  "catmull-rom",
  "cubic",
  "mitchell",
  "lanczos2",
  "lanczos3",
]);
const CHANNELS = { red: 0, green: 1, blue: 2, alpha: 3 };

// Passed as the constructor's first argument to build a derived image from the
// parts of another. Module-private, so guest code cannot reach that path.
const DERIVE = Symbol("derive");

// Pixels' stable error codes, as this module names them. Bun's names where Bun
// has one; a bad argument Pixels finds late (a crop outside the image) is the
// RangeError it would have been had it been found at the call.
const ERROR_CODES = {
  malformed: "ERR_IMAGE_DECODE_FAILED",
  unsupported: "ERR_IMAGE_FORMAT_UNSUPPORTED",
  limit_exceeded: "ERR_IMAGE_TOO_LARGE",
};

// Bun options Pixels has no encoder support for. Refused by name rather than
// ignored, so a program ported from Bun learns at the call, not from the file.
const UNSUPPORTED_OPTIONS = {
  jpeg: ["progressive"],
  png: ["compressionLevel", "palette", "colors", "dither"],
};

function fail(result) {
  const code = ERROR_CODES[result.error];
  if (code !== undefined) {
    const err = new Error(result.message);
    err.code = code;
    return err;
  }
  if (result.error === "invalid_argument") return new RangeError(result.message);
  return new Error(result.message);
}

function settle(result) {
  if ("error" in result) throw fail(result);
  return result.ok;
}

function optionsObject(options, what) {
  if (options === undefined) return {};
  if (options === null || typeof options !== "object") {
    throw new TypeError(`${what} options must be an object`);
  }
  return options;
}

function isFile(value) {
  return value !== null && typeof value === "object" && typeof value[FILE_BODY] === "function";
}

// The input, as either `{ bytes }` or `{ path }`. A view is kept, not copied:
// the op call copies it, so the bytes used are the ones there at the terminal.
function sourceOf(input) {
  if (typeof input === "string" || input instanceof URL) {
    throw new TypeError("an Image takes bytes, a Blob or a file(); pass file(path) from runtime:fs to read a file");
  }
  if (isFile(input)) return { path: input[FILE_BODY]() };
  if (input instanceof Blob) return { bytes: input[BLOB_BYTES]() };
  let buffer;
  if (input instanceof ArrayBuffer) buffer = input;
  else if (ArrayBuffer.isView(input)) buffer = input.buffer;
  else throw new TypeError("an Image takes a Uint8Array, an ArrayBuffer, a view, a Blob or a file()");
  if (typeof SharedArrayBuffer === "function" && buffer instanceof SharedArrayBuffer) {
    throw new TypeError("an Image cannot read a SharedArrayBuffer, which may change while it is read");
  }
  if (buffer.resizable) {
    throw new TypeError("an Image cannot read a resizable ArrayBuffer");
  }
  if (input instanceof ArrayBuffer) return { bytes: new Uint8Array(input) };
  return { bytes: new Uint8Array(input.buffer, input.byteOffset, input.byteLength) };
}

function openOptions(options) {
  const o = optionsObject(options, "Image");
  const open = {};
  if (o.maxPixels !== undefined) {
    if (!Number.isInteger(o.maxPixels) || o.maxPixels < 1) {
      throw new RangeError("maxPixels must be a positive integer");
    }
    open.maxPixels = o.maxPixels;
  }
  for (const key of ["autoOrient", "toSrgb", "animated"]) {
    if (o[key] !== undefined) open[key] = Boolean(o[key]);
  }
  return open;
}

function size(value, what) {
  if (value === undefined || value === null) return 0;
  if (!Number.isInteger(value) || value < 1) throw new RangeError(`${what} must be a positive integer`);
  return value;
}

function offset(value, what) {
  if (!Number.isInteger(value) || value < 0) throw new RangeError(`${what} must be a non-negative integer`);
  return value;
}

function finite(value, what) {
  if (typeof value !== "number" || !Number.isFinite(value)) throw new TypeError(`${what} must be a finite number`);
  return value;
}

function byte(value, what) {
  if (!Number.isInteger(value) || value < 0 || value > 255) throw new RangeError(`${what} must be an integer from 0 to 255`);
  return value;
}

// "#rgb", "#rgba", "#rrggbb", "#rrggbbaa", or sharp's `{ r, g, b, alpha }`
// with alpha from 0 to 1. Returns [r, g, b, a] bytes.
function colour(value, what) {
  if (typeof value === "string") {
    const hex = /^#([0-9a-f]{3,4}|[0-9a-f]{6}|[0-9a-f]{8})$/i.exec(value)?.[1];
    if (hex === undefined) throw new TypeError(`${what} must be a #rgb, #rgba, #rrggbb or #rrggbbaa colour`);
    const digits = hex.length <= 4 ? [...hex].map((d) => d + d) : hex.match(/../g);
    const [r, g, b, a = "ff"] = digits;
    return [r, g, b, a].map((d) => parseInt(d, 16));
  }
  if (value !== null && typeof value === "object") {
    const { r = 0, g = 0, b = 0, alpha = 1 } = value;
    if (typeof alpha !== "number" || !(alpha >= 0 && alpha <= 1)) throw new RangeError(`${what}.alpha must be from 0 to 1`);
    return [byte(r, `${what}.r`), byte(g, `${what}.g`), byte(b, `${what}.b`), Math.round(alpha * 255)];
  }
  throw new TypeError(`${what} must be a colour string or { r, g, b, alpha }`);
}

function quality(value) {
  if (value === undefined) return undefined;
  if (!Number.isInteger(value) || value < 1 || value > 100) throw new RangeError("quality must be an integer from 1 to 100");
  return value;
}

function refuseUnsupported(format, o) {
  for (const key of UNSUPPORTED_OPTIONS[format] ?? []) {
    if (o[key] !== undefined) throw new TypeError(`${format}({ ${key} }) is not supported`);
  }
}

const MIME = {
  jpeg: "image/jpeg",
  png: "image/png",
  webp: "image/webp",
  avif: "image/avif",
  gif: "image/gif",
  tiff: "image/tiff",
};

/**
 * A lazy image pipeline. Every transform and format method returns a new
 * `Image`; nothing is decoded until `metadata()` or a terminal is awaited.
 */
export class Image {
  #source;
  #open;
  #steps;
  #output;

  /**
   * `input` is a Uint8Array, an ArrayBuffer, any view, a Blob, or a
   * runtime:fs `file()`. Options: `maxPixels` (268 MP), `autoOrient` (true),
   * `toSrgb` (true), `animated` (false).
   */
  constructor(input, options) {
    if (input === DERIVE) {
      [this.#source, this.#open, this.#steps, this.#output] = options;
      return;
    }
    this.#source = sourceOf(input);
    this.#open = openOptions(options);
    this.#steps = [];
    this.#output = null;
  }

  #with(step, output = this.#output) {
    const steps = step === null ? this.#steps : [...this.#steps, step];
    return new Image(DERIVE, [this.#source, this.#open, steps, output]);
  }

  #call(kind, ...rest) {
    const fromFile = this.#source.path !== undefined;
    const name = `image_${kind}${fromFile ? "_file" : ""}`;
    return ops[name](fromFile ? this.#source.path : this.#source.bytes, this.#open, this.#steps, ...rest);
  }

  /**
   * Resize to `width` × `height`. Omit either (or pass null) to keep the
   * aspect ratio. Options: `fit` ("fill" | "inside" | "outside" | "cover" |
   * "contain", default "fill"), `filter` (default "lanczos3"), `background`
   * for "contain" (default transparent), `withoutEnlargement`.
   */
  resize(width, height, options) {
    const w = size(width, "width");
    const h = size(height, "height");
    if (w === 0 && h === 0) throw new TypeError("resize needs a width or a height");
    const o = optionsObject(options, "resize");
    const fit = o.fit ?? "fill";
    if (!FITS.has(fit)) throw new TypeError(`fit must be one of ${[...FITS].join(", ")}`);
    const filter = o.filter ?? "lanczos3";
    if (!FILTERS.has(filter)) throw new TypeError(`filter must be one of ${[...FILTERS].join(", ")}`);
    const background = o.background === undefined ? [0, 0, 0, 0] : colour(o.background, "background");
    return this.#with(["resize", w, h, fit, filter, background, Boolean(o.withoutEnlargement)]);
  }

  /** Keep only the region `{ left, top, width, height }`. */
  crop(region) {
    const r = optionsObject(region, "crop");
    return this.#with([
      "crop",
      offset(r.left ?? 0, "left"),
      offset(r.top ?? 0, "top"),
      size(r.width, "width") || missing("width"),
      size(r.height, "height") || missing("height"),
    ]);
  }

  /** Rotate clockwise by a multiple of 90 degrees. */
  rotate(degrees) {
    if (!Number.isInteger(degrees) || degrees % 90 !== 0) {
      throw new RangeError("rotate takes a multiple of 90 degrees");
    }
    return this.#with(["rotate", ((degrees % 360) + 360) % 360]);
  }

  /** Mirror top to bottom. */
  flip() {
    return this.#with(["flip"]);
  }

  /** Mirror left to right. */
  flop() {
    return this.#with(["flop"]);
  }

  /**
   * Adjust `brightness` and `saturation` (multipliers; 1 is unchanged, a
   * saturation of 0 is greyscale) and rotate `hue` by degrees.
   */
  modulate(options) {
    const o = optionsObject(options, "modulate");
    const brightness = finite(o.brightness ?? 1, "brightness");
    const saturation = finite(o.saturation ?? 1, "saturation");
    if (brightness < 0 || saturation < 0) throw new RangeError("brightness and saturation must not be negative");
    return this.#with(["modulate", brightness, saturation, finite(o.hue ?? 0, "hue")]);
  }

  /** Gaussian blur of standard deviation `sigma` pixels. */
  blur(sigma) {
    if (finite(sigma, "sigma") <= 0) throw new RangeError("sigma must be positive");
    return this.#with(["blur", sigma]);
  }

  /** Sharpen; `amount` scales the effect (default 1). */
  sharpen(amount = 1) {
    return this.#with(["sharpen", finite(amount, "amount")]);
  }

  /** Drop transparency by drawing over `background` (default black). */
  flatten(options) {
    const o = optionsObject(options, "flatten");
    const [r, g, b] = o.background === undefined ? [0, 0, 0] : colour(o.background, "background");
    return this.#with(["flatten", r, g, b]);
  }

  /** Convert to greyscale, keeping any alpha. */
  grayscale() {
    return this.#with(["grayscale"]);
  }

  /** One channel as a greyscale image: 0–3, or "red" | "green" | "blue" | "alpha". */
  extractChannel(channel) {
    const index = typeof channel === "string" ? CHANNELS[channel] : channel;
    if (!Number.isInteger(index) || index < 0 || index > 3) {
      throw new RangeError("channel must be 0–3 or red, green, blue or alpha");
    }
    return this.#with(["extractChannel", index]);
  }

  /** Encode as JPEG. `quality` 1–100, default 80. */
  jpeg(options) {
    const o = optionsObject(options, "jpeg");
    refuseUnsupported("jpeg", o);
    return this.#with(null, { format: "jpeg", quality: quality(o.quality) });
  }

  /** Encode as PNG. */
  png(options) {
    refuseUnsupported("png", optionsObject(options, "png"));
    return this.#with(null, { format: "png" });
  }

  /** Encode as WebP: lossy at `quality` (default 80), or `lossless`. */
  webp(options) {
    const o = optionsObject(options, "webp");
    return this.#with(null, { format: "webp", quality: quality(o.quality), lossless: Boolean(o.lossless) });
  }

  /** Encode as AVIF (8-bit 4:2:0) at `quality`, default 80. */
  avif(options) {
    const o = optionsObject(options, "avif");
    if (o.lossless) throw new TypeError("avif({ lossless }) is not supported");
    return this.#with(null, { format: "avif", quality: quality(o.quality) });
  }

  /** Encode as a single-frame GIF. */
  gif() {
    return this.#with(null, { format: "gif" });
  }

  /** Encode as TIFF. */
  tiff() {
    return this.#with(null, { format: "tiff" });
  }

  /**
   * `{ width, height, format, pixelFormat, hasAlpha, animation }` at this
   * point in the chain, read from the header without decoding pixels.
   * `format` is the source's; `animation` is `{ frames, loop, durations }`
   * or null.
   */
  async metadata() {
    return settle(await this.#call("metadata"));
  }

  /** The encoded image. Without a format method, in the source's format. */
  async bytes() {
    return settle(await this.#call("encode", this.#output)).bytes;
  }

  /** The encoded image as a Blob whose `type` is its media type. */
  async blob() {
    const { bytes, format } = settle(await this.#call("encode", this.#output));
    return new Blob([bytes], { type: MIME[format] });
  }

  /**
   * Encode into `destination`, a runtime:fs `file()`; resolves to the number
   * of bytes written. The format is the one chained, or the source's — never
   * the destination's extension.
   */
  async write(destination) {
    if (!isFile(destination)) {
      throw new TypeError("write takes a file() from runtime:fs");
    }
    return settle(await this.#call("write", this.#output, destination[FILE_BODY]()));
  }

  get [Symbol.toStringTag]() {
    return "Image";
  }
}

function missing(what) {
  throw new TypeError(`crop needs a ${what}`);
}

export default { Image };
