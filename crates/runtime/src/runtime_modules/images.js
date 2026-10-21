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

// sharp's nine crop gravities and their aliases. Saliency strategies
// ("attention", "entropy") are refused by name in resize: Pixels crops to a
// gravity, it does not find a subject.
const POSITIONS = new Set([
  "center",
  "centre",
  "north",
  "top",
  "northeast",
  "right top",
  "east",
  "right",
  "southeast",
  "right bottom",
  "south",
  "bottom",
  "southwest",
  "left bottom",
  "west",
  "left",
  "northwest",
  "left top",
]);

// Passed as the constructor's first argument to build a derived image from the
// parts of another. Module-private, so guest code cannot reach that path.
const DERIVE = Symbol("derive");

// Pixels' stable error codes, under the names Bun gives them, so code that
// branches on them moves unchanged; a bad argument Pixels finds late (a crop outside the image) is the
// RangeError it would have been had it been found at the call.
const ERROR_CODES = {
  malformed: "ERR_IMAGE_DECODE_FAILED",
  unsupported: "ERR_IMAGE_FORMAT_UNSUPPORTED",
  limit_exceeded: "ERR_IMAGE_TOO_MANY_PIXELS",
};

// sharp's 25 blend modes; the colour ones in both spellings.
const BLENDS = new Set([
  "clear",
  "source",
  "over",
  "in",
  "out",
  "atop",
  "dest",
  "dest-over",
  "dest-in",
  "dest-out",
  "dest-atop",
  "xor",
  "add",
  "saturate",
  "multiply",
  "screen",
  "overlay",
  "darken",
  "lighten",
  "colour-dodge",
  "color-dodge",
  "colour-burn",
  "color-burn",
  "hard-light",
  "soft-light",
  "difference",
  "exclusion",
]);
const EXTEND_WITH = new Set(["background", "copy", "repeat", "mirror"]);
// sharp's sample types that Pixels has: 8-bit, 16-bit and float.
const DEPTHS = new Set(["uchar", "ushort", "float"]);
const CHROMA = new Set(["4:2:0", "4:4:4"]);

// sharp options Pixels has no support for. Refused by name rather than
// ignored, so a program ported from sharp learns at the call, not from the file.
const UNSUPPORTED_OPTIONS = {
  png: ["progressive", "quality", "effort"],
  avif: ["lossless"],
  raw: ["premultiplied"],
  create: ["noise", "pageHeight"],
  convolve: ["scale", "offset"],
  jxl: ["distance", "effort"],
  trim: ["margin"],
  withMetadata: ["orientation"],
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
  if (isFile(input)) return { path: input[FILE_BODY](), file: input };
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
  if (o.page !== undefined) {
    if (!Number.isInteger(o.page) || o.page < 0) {
      throw new RangeError("page must be a non-negative integer");
    }
    open.page = o.page;
  }
  if (o.raw !== undefined) open.raw = layout(o.raw, "raw", [1, 2, 3, 4]);
  return open;
}

function depth(value) {
  if (value === undefined) return "uchar";
  if (!DEPTHS.has(value)) throw new TypeError("depth must be uchar, ushort or float");
  return value;
}

// `{ width, height, channels, depth }` for raw pixels or a created canvas.
function layout(value, what, channels) {
  const o = optionsObject(value, what);
  refuseUnsupported(what, o);
  if (!channels.includes(o.channels)) {
    throw new RangeError(`${what}.channels must be ${channels.join(", ").replace(/, (\d)$/, " or $1")}`);
  }
  return {
    width: size(o.width, `${what}.width`) || missingOf(what, "width"),
    height: size(o.height, `${what}.height`) || missingOf(what, "height"),
    channels: o.channels,
    depth: depth(o.depth),
  };
}

function missingOf(what, key) {
  throw new TypeError(`${what} needs a ${key}`);
}

// sharp's `{ create: { width, height, channels, background } }`: a canvas
// of one colour, 8-bit RGB or RGBA.
function isCreate(input) {
  return input !== null && typeof input === "object" && Object.getPrototypeOf(input) === Object.prototype && "create" in input;
}

function createOptions(create) {
  const o = optionsObject(create, "create");
  const open = layout({ ...o, depth: undefined, background: undefined }, "create", [3, 4]);
  if (o.background === undefined) throw new TypeError("create needs a background");
  open.background = colour(o.background, "background");
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

// A `linear` side: one finite number, or one per channel.
function linearSide(value, what) {
  if (typeof value === "number") {
    finite(value, what);
    return [value, value, value];
  }
  if (Array.isArray(value) && value.length === 3) {
    return value.map((v) => finite(v, what));
  }
  throw new TypeError(`${what} must be a number or three numbers`);
}

function refuseUnsupported(method, o) {
  for (const key of UNSUPPORTED_OPTIONS[method] ?? []) {
    if (o[key] !== undefined) throw new TypeError(`${method}({ ${key} }) is not supported`);
  }
}

function integer(value, what, min, max) {
  if (value === undefined) return undefined;
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new RangeError(`${what} must be an integer from ${min} to ${max}`);
  }
  return value;
}

function chroma(value) {
  if (value === undefined) return undefined;
  if (!CHROMA.has(value)) throw new TypeError('chromaSubsampling must be "4:2:0" or "4:4:4"');
  return value;
}

// A palette's size and dithering, shared by png() and gif(). `colors` is
// sharp's other spelling and Bun's only one; Bun's `dither` is a boolean,
// sharp's a level from 0 to 1, and both are taken.
function paletteOptions(o, what) {
  if (o.colours !== undefined && o.colors !== undefined && o.colours !== o.colors) {
    throw new TypeError(`${what}: colours and colors disagree`);
  }
  const out = {};
  const colours = integer(o.colours ?? o.colors, "colours", 2, 256);
  if (colours !== undefined) out.colours = colours;
  if (typeof o.dither === "boolean") out.dither = o.dither ? 1 : 0;
  else if (o.dither !== undefined) {
    if (typeof o.dither !== "number" || !(o.dither >= 0 && o.dither <= 1)) {
      throw new RangeError("dither must be from 0 to 1, or a boolean");
    }
    out.dither = o.dither;
  }
  return out;
}

// Copies an option object, leaving out what is undefined.
function defined(object) {
  return Object.fromEntries(Object.entries(object).filter(([, v]) => v !== undefined));
}

function integerOffset(value, what) {
  if (!Number.isInteger(value)) throw new TypeError(`${what} must be an integer`);
  return value;
}

function margin(value, what) {
  if (value === undefined) return 0;
  return offset(value, what);
}

// An overlay as `composite` records it: its pipeline (an Image's, or a new
// one's) and sharp's placement options. Resolved to bytes at the terminal.
function overlayOf(entry) {
  const o = optionsObject(entry, "composite");
  if (!("input" in o)) throw new TypeError("a composite overlay needs an input");
  const image = o.input instanceof Image ? o.input : new Image(o.input, o.raw === undefined ? undefined : { raw: o.raw });
  const blend = o.blend ?? "over";
  if (!BLENDS.has(blend)) throw new TypeError(`blend must be one of ${[...BLENDS].join(", ")}`);
  const gravity = o.gravity ?? "center";
  if (!POSITIONS.has(gravity)) {
    throw new TypeError("gravity must be center, north, northeast, east, southeast, south, southwest, west or northwest");
  }
  if ((o.left === undefined) !== (o.top === undefined)) {
    throw new TypeError("a composite overlay needs both left and top, or neither");
  }
  const placement = { blend, gravity, tile: Boolean(o.tile) };
  if (o.left !== undefined) {
    placement.left = integerOffset(o.left, "left");
    placement.top = integerOffset(o.top, "top");
  }
  return { image, placement };
}

const MIME = {
  jpeg: "image/jpeg",
  png: "image/png",
  webp: "image/webp",
  avif: "image/avif",
  gif: "image/gif",
  tiff: "image/tiff",
  jxl: "image/jxl",
  raw: "application/octet-stream",
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
  #keep;

  /**
   * `input` is a Uint8Array, an ArrayBuffer, any view, a Blob, or a
   * runtime:fs `file()`. Options: `maxPixels` (268 MP), `autoOrient` (true),
   * `toSrgb` (true), `animated` (false), `page` (0: the first page of a
   * multi-page TIFF). JPEG, PNG, WebP, AVIF, GIF, TIFF and JPEG XL are
   * read.
   */
  constructor(input, options) {
    if (input === DERIVE) {
      [this.#source, this.#open, this.#steps, this.#output, this.#keep] = options;
      return;
    }
    if (isCreate(input)) {
      this.#source = { bytes: new Uint8Array(0) };
      this.#open = { ...openOptions(options), create: createOptions(input.create) };
      if (this.#open.raw !== undefined) throw new TypeError("an Image is created or raw, not both");
    } else {
      this.#source = sourceOf(input);
      this.#open = openOptions(options);
    }
    this.#steps = [];
    this.#output = null;
    this.#keep = {};
  }

  #with(step, output = this.#output, keep = this.#keep) {
    const steps = step === null ? this.#steps : [...this.#steps, step];
    return new Image(DERIVE, [this.#source, this.#open, steps, output, keep]);
  }

  #keeping(keep) {
    return this.#with(null, this.#output, { ...this.#keep, ...keep });
  }

  // The steps with each composite overlay resolved to `{ bytes, open, steps }`
  // and its placement. A file() overlay is read here, through runtime:fs, so
  // it needs FileRead exactly as reading that file would; the op itself then
  // reads nothing but its own source. Every byte source is copied (and every
  // file read started) synchronously, so the bytes used are the ones there
  // when the terminal was called, as without overlays.
  #snapshotSteps(pending) {
    return this.#steps.map((step) => {
      if (step[0] !== "composite") return step;
      return ["composite", step[1].map(({ image, placement }) => image.#snapshot(pending, placement))];
    });
  }

  // `placement` is filled in, not spread, so a file's bytes land in the
  // object the op receives.
  #snapshot(pending, placement) {
    const source = this.#source;
    const overlay = { ...placement, open: this.#open, steps: this.#snapshotSteps(pending) };
    if (source.file !== undefined) {
      pending.push(source.file.bytes().then((bytes) => (overlay.bytes = bytes)));
    } else {
      overlay.bytes = source.bytes.slice();
    }
    return overlay;
  }

  #call(kind, ...rest) {
    const fromFile = this.#source.path !== undefined;
    const name = `image_${kind}${fromFile ? "_file" : ""}`;
    const invoke = (source, steps) => ops[name](source, this.#open, steps, ...rest);
    if (!this.#steps.some((step) => step[0] === "composite")) {
      return invoke(fromFile ? this.#source.path : this.#source.bytes, this.#steps);
    }
    const pending = [];
    const steps = this.#snapshotSteps(pending);
    const source = fromFile ? this.#source.path : this.#source.bytes.slice();
    return Promise.all(pending).then(() => invoke(source, steps));
  }

  #encoding() {
    return { ...this.#output, ...this.#keep };
  }

  /**
   * Resize to `width` × `height`. Omit either (or pass null) to keep the
   * aspect ratio. Options: `fit` ("fill" | "inside" | "outside" | "cover" |
   * "contain", default "fill"), `filter` (default "lanczos3"), `background`
   * for "contain" (default transparent), `withoutEnlargement`,
   * `withoutReduction`, and `position` for "cover" and "contain" (a gravity:
   * "center" (default), "north", "northeast", "east", "southeast", "south",
   * "southwest", "west" or "northwest").
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
    const position = o.position ?? "center";
    if (!POSITIONS.has(position)) {
      if (position === "attention" || position === "entropy") {
        throw new TypeError('position "attention" and "entropy" need saliency, which is not supported');
      }
      throw new TypeError(
        'position must be center, north, northeast, east, southeast, south, southwest, west or northwest',
      );
    }
    return this.#with([
      "resize",
      w,
      h,
      fit,
      filter,
      background,
      Boolean(o.withoutEnlargement),
      position,
      Boolean(o.withoutReduction),
    ]);
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
   * saturation of 0 is greyscale), rotate `hue` by degrees, and shift
   * `lightness` (added units, 0 is unchanged).
   */
  modulate(options) {
    const o = optionsObject(options, "modulate");
    const brightness = finite(o.brightness ?? 1, "brightness");
    const saturation = finite(o.saturation ?? 1, "saturation");
    if (brightness < 0 || saturation < 0) throw new RangeError("brightness and saturation must not be negative");
    return this.#with([
      "modulate",
      brightness,
      saturation,
      finite(o.hue ?? 0, "hue"),
      finite(o.lightness ?? 0, "lightness"),
    ]);
  }

  /** Gaussian blur of standard deviation `sigma` pixels. */
  blur(sigma) {
    if (finite(sigma, "sigma") <= 0) throw new RangeError("sigma must be positive");
    return this.#with(["blur", sigma]);
  }

  /**
   * Sharpen; `amount` scales the effect (default 1). sharp's
   * `sharpen({ sigma, m1, m2, … })` is not supported.
   */
  sharpen(amount = 1) {
    if (amount !== null && typeof amount === "object") {
      throw new TypeError("sharpen({ sigma, m1, m2, ... }) is not supported; pass an amount, or convolve a kernel");
    }
    return this.#with(["sharpen", finite(amount, "amount")]);
  }

  /**
   * Convolve with `kernel`, `width` × `height` finite numbers in row order
   * (each side odd, at most 63). The result is divided by the kernel's sum,
   * unless that sum is 0. sharp's `scale` and `offset` are not supported.
   */
  convolve(options) {
    const o = optionsObject(options, "convolve");
    refuseUnsupported("convolve", o);
    const side = (value, what) => {
      if (!Number.isInteger(value) || value < 1 || value > 63 || value % 2 === 0) {
        throw new RangeError(`${what} must be an odd integer from 1 to 63`);
      }
      return value;
    };
    const width = side(o.width, "width");
    const height = side(o.height, "height");
    if (!Array.isArray(o.kernel) || o.kernel.length !== width * height) {
      throw new TypeError("kernel must be an array of width × height numbers");
    }
    return this.#with(["convolve", width, height, o.kernel.map((v) => finite(v, "kernel"))]);
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

  /**
   * Greyscale by luma, keeping the channel count: RGB becomes three equal
   * channels. `grayscale()` instead makes a one-channel image.
   */
  greyscale() {
    return this.#with(["greyscale"]);
  }

  /** One channel as a greyscale image: 0–3, or "red" | "green" | "blue" | "alpha". */
  extractChannel(channel) {
    if (typeof channel === "string") {
      if (!(channel in CHANNELS)) {
        throw new RangeError("channel must be 0–3 or red, green, blue or alpha");
      }
      return this.#with(["extractChannelNamed", channel]);
    }
    if (!Number.isInteger(channel) || channel < 0 || channel > 3) {
      throw new RangeError("channel must be 0–3 or red, green, blue or alpha");
    }
    return this.#with(["extractChannel", channel]);
  }

  /** Invert every channel. `alpha` (default true) inverts alpha too. */
  negate(options) {
    const o = optionsObject(options, "negate");
    return this.#with(["negate", Boolean(o.alpha ?? true)]);
  }

  /**
   * Gamma correction with `exponent`: each channel becomes
   * `value^(1 / exponent)`, so above 1 brightens. This is libvips'
   * definition, not sharp's two-argument `gamma(gamma, gammaOut)`.
   */
  gamma(exponent) {
    if (finite(exponent, "exponent") <= 0) throw new RangeError("exponent must be positive");
    return this.#with(["gamma", exponent]);
  }

  /** `value * a + b` on every channel. Each side is a number or three. */
  linear(a, b = 0) {
    return this.#with(["linear", linearSide(a, "a"), linearSide(b, "b")]);
  }

  /**
   * Two levels: at or above `level` (default 128) becomes full, below
   * becomes zero. `greyscale` (default true) thresholds luma into every
   * channel; without it, each channel on its own. Unlike sharp's, the
   * channel count is kept.
   */
  threshold(level = 128, options) {
    if (!Number.isInteger(level) || level < 0 || level > 255) {
      throw new RangeError("level must be an integer from 0 to 255");
    }
    const o = optionsObject(options, "threshold");
    return this.#with(["threshold", level, o.greyscale ?? true]);
  }

  /** Tint towards a colour, keeping each pixel's luma. */
  tint(color) {
    const [r, g, b] = colour(color, "color");
    return this.#with(["tint", r, g, b]);
  }

  /** Mix channels through a 3×3 or 4×4 matrix of finite numbers. */
  recomb(matrix) {
    if (!Array.isArray(matrix) || (matrix.length !== 3 && matrix.length !== 4)) {
      throw new TypeError("recomb takes a 3x3 or 4x4 matrix");
    }
    const flat = [];
    for (const row of matrix) {
      if (!Array.isArray(row) || row.length !== matrix.length) {
        throw new TypeError("recomb takes a 3x3 or 4x4 matrix");
      }
      for (const value of row) flat.push(finite(value, "matrix"));
    }
    return this.#with(["recomb", matrix.length, flat]);
  }

  /** Add an alpha channel at `alpha` (default opaque) if there is none. */
  ensureAlpha(alpha = 1) {
    if (typeof alpha !== "number" || !(alpha >= 0 && alpha <= 1)) {
      throw new RangeError("alpha must be from 0 to 1");
    }
    return this.#with(["ensureAlpha", alpha]);
  }

  /** Drop the alpha channel without compositing. */
  removeAlpha() {
    return this.#with(["removeAlpha"]);
  }

  /**
   * Grow the canvas: `n` pixels on every side, or `{ top, right, bottom,
   * left }`. `extendWith` fills the margin with `"background"` (default),
   * `"copy"` (the edge), `"repeat"` or `"mirror"`; `background` defaults to
   * opaque black, and a translucent one adds alpha.
   */
  extend(extend) {
    if (typeof extend === "number") {
      const n = offset(extend, "extend");
      return this.#with(["extend", n, n, n, n, "background", [0, 0, 0, 255]]);
    }
    const o = optionsObject(extend, "extend");
    const extendWith = o.extendWith ?? "background";
    if (!EXTEND_WITH.has(extendWith)) {
      throw new TypeError("extendWith must be background, copy, repeat or mirror");
    }
    return this.#with([
      "extend",
      margin(o.top, "top"),
      margin(o.right, "right"),
      margin(o.bottom, "bottom"),
      margin(o.left, "left"),
      extendWith,
      o.background === undefined ? [0, 0, 0, 255] : colour(o.background, "background"),
    ]);
  }

  /**
   * Cut away a border near `background` (default: the top-left pixel's
   * colour), within `threshold` (default 10). `lineArt` keeps one-pixel
   * lines. Runs the chain before it in memory, so trim after shrinking.
   */
  trim(options) {
    const o = optionsObject(options, "trim");
    refuseUnsupported("trim", o);
    const threshold = finite(o.threshold ?? 10, "threshold");
    if (threshold < 0) throw new RangeError("threshold must not be negative");
    const background = o.background === undefined ? null : colour(o.background, "background");
    return this.#with(["trim", background, threshold, Boolean(o.lineArt)]);
  }

  /**
   * Draw images over this one, the first lowest. Each is `{ input, blend,
   * gravity, left, top, tile }`: `input` an Image or what one takes, `blend`
   * one of sharp's modes (default "over"), `gravity` where it sits (default
   * "center") unless `left` and `top` place it, and `tile` repeats it.
   */
  composite(overlays) {
    if (!Array.isArray(overlays)) throw new TypeError("composite takes an array of overlays");
    return this.#with(["composite", overlays.map(overlayOf)]);
  }

  /** Copy the source's EXIF into the output, which strips it by default. */
  keepExif() {
    return this.#keeping({ keepExif: true });
  }

  /** Copy the source's XMP into the output, which strips it by default. */
  keepXmp() {
    return this.#keeping({ keepXmp: true });
  }

  /** Copy the source's EXIF and XMP. The ICC profile is always kept. */
  keepMetadata() {
    return this.#keeping({ keepExif: true, keepXmp: true });
  }

  /** Write `xmp`, an XMP packet, in place of the source's. */
  withXmp(xmp) {
    if (typeof xmp !== "string") throw new TypeError("withXmp takes a string");
    return this.#keeping({ keepXmp: true, xmp: new TextEncoder().encode(xmp) });
  }

  /** Keep EXIF and XMP, and with `density` write that resolution in DPI. */
  withMetadata(options) {
    const o = optionsObject(options, "withMetadata");
    refuseUnsupported("withMetadata", o);
    const keep = { keepExif: true, keepXmp: true };
    if (o.density !== undefined) {
      if (finite(o.density, "density") <= 0) throw new RangeError("density must be positive");
      keep.density = o.density;
    }
    return this.#keeping(keep);
  }

  /**
   * Encode as JPEG: `quality` 1–100 (default 80), `progressive`, and
   * `chromaSubsampling` ("4:2:0", or "4:4:4"; by default 4:4:4 from
   * quality 90).
   */
  jpeg(options) {
    const o = optionsObject(options, "jpeg");
    return this.#with(
      null,
      defined({
        format: "jpeg",
        quality: quality(o.quality),
        progressive: o.progressive === undefined ? undefined : Boolean(o.progressive),
        chromaSubsampling: chroma(o.chromaSubsampling),
      }),
    );
  }

  /**
   * Encode as PNG: `compressionLevel` 0–9, or a palette image with
   * `palette`, `colours` (2–256) and `dither` (0–1); either of the last two
   * implies a palette.
   */
  png(options) {
    const o = optionsObject(options, "png");
    refuseUnsupported("png", o);
    return this.#with(
      null,
      defined({
        format: "png",
        compressionLevel: integer(o.compressionLevel, "compressionLevel", 0, 9),
        palette: o.palette === undefined ? undefined : Boolean(o.palette),
        ...paletteOptions(o, "png"),
      }),
    );
  }

  /**
   * Encode as WebP: lossy at `quality` (default 80), or `lossless`, or
   * `nearLossless`; `effort` 0–6 (default 4) and lossy `alphaQuality` 0–100.
   */
  webp(options) {
    const o = optionsObject(options, "webp");
    return this.#with(
      null,
      defined({
        format: "webp",
        quality: quality(o.quality),
        lossless: Boolean(o.lossless),
        nearLossless: o.nearLossless === undefined ? undefined : Boolean(o.nearLossless),
        effort: integer(o.effort, "effort", 0, 6),
        alphaQuality: integer(o.alphaQuality, "alphaQuality", 0, 100),
      }),
    );
  }

  /**
   * Encode as 8-bit AVIF at `quality` (default 80): `chromaSubsampling`
   * "4:2:0" (default) or "4:4:4", and `effort` 0–9 (default 4).
   */
  avif(options) {
    const o = optionsObject(options, "avif");
    refuseUnsupported("avif", o);
    return this.#with(
      null,
      defined({
        format: "avif",
        quality: quality(o.quality),
        chromaSubsampling: chroma(o.chromaSubsampling),
        effort: integer(o.effort, "effort", 0, 9),
      }),
    );
  }

  /** Encode as a single-frame GIF of `colours` (2–256) with `dither` (0–1). */
  gif(options) {
    return this.#with(null, { format: "gif", ...paletteOptions(optionsObject(options, "gif"), "gif") });
  }

  /** Encode as TIFF. */
  tiff() {
    return this.#with(null, { format: "tiff" });
  }

  /**
   * Raw pixels, packed, no header: 1 to 4 channels as the image has them,
   * of `depth` "uchar" (default), "ushort" or "float". `metadata()` of the
   * same chain gives the width, height and channels.
   */
  raw(options) {
    const o = optionsObject(options, "raw");
    return this.#with(null, { format: "raw", depth: depth(o.depth) });
  }

  /** Encode as JPEG XL: lossy at `quality` (default 80), or `lossless`. */
  jxl(options) {
    const o = optionsObject(options, "jxl");
    refuseUnsupported("jxl", o);
    return this.#with(null, defined({ format: "jxl", quality: quality(o.quality), lossless: Boolean(o.lossless) }));
  }

  /**
   * The image at this point in the chain, read from headers without
   * decoding pixels: `width`, `height`, `pixelFormat`, `channels` and
   * `hasAlpha` as the chain leaves it, `trimOffsetLeft`/`trimOffsetTop`
   * after a trim, and what the source says about itself under sharp's
   * names — `format`, `animation`, `pages`, `orientation`, `density`,
   * `isProgressive`, `isPalette`, `icc`, `exif`, `xmp` and the rest. A
   * field the file does not state is absent.
   */
  async metadata() {
    return settle(await this.#call("metadata"));
  }

  /**
   * sharp's pixel statistics of the result: per channel `min`, `max`,
   * `sum`, `squaresSum`, `mean`, `stdev` and where the extremes are, and
   * `isOpaque`, `entropy`, `sharpness` and the `dominant` colour. Runs the
   * chain.
   */
  async stats() {
    return settle(await this.#call("stats"));
  }

  /** The encoded image. Without a format method, in the source's format. */
  async bytes() {
    return settle(await this.#call("encode", this.#encoding())).bytes;
  }

  /** The encoded image as a Blob whose `type` is its media type. */
  async blob() {
    const { bytes, format } = settle(await this.#call("encode", this.#encoding()));
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
    return settle(await this.#call("write", this.#encoding(), destination[FILE_BODY]()));
  }

  get [Symbol.toStringTag]() {
    return "Image";
  }
}

function missing(what) {
  throw new TypeError(`crop needs a ${what}`);
}

export default { Image };
