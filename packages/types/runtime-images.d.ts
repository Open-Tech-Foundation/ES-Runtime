declare module "runtime:images" {
  import type { FsFile } from "runtime:fs";

  /** What an image is read from. A path string is refused: pass `file(path)`. */
  export type ImageInput = ArrayBuffer | ArrayBufferView | Blob | FsFile;

  /** How the image is opened. */
  export interface ImageOptions {
    /** An image over this many pixels fails at its header. Default 268402689. */
    maxPixels?: number;
    /** Apply the orientation the file declares. Default true. */
    autoOrient?: boolean;
    /** Convert an embedded ICC profile to sRGB. Default true. */
    toSrgb?: boolean;
    /**
     * Ask for every frame. Not yet supported: an animated input with this set
     * fails with `ERR_IMAGE_FORMAT_UNSUPPORTED`. Default false.
     */
    animated?: boolean;
    /** The page to open of a multi-page TIFF. Default 0. */
    page?: number;
  }

  /** `"#rgb"`, `"#rgba"`, `"#rrggbb"`, `"#rrggbbaa"`, or channels with alpha from 0 to 1. */
  export type Colour = string | { r?: number; g?: number; b?: number; alpha?: number };

  export type Fit = "fill" | "inside" | "outside" | "cover" | "contain";

  export type ResizeFilter =
    | "lanczos3"
    | "lanczos2"
    | "mitchell"
    | "catmull-rom"
    | "cubic"
    | "bilinear"
    | "linear"
    | "box"
    | "nearest";

  export type Position =
    | "center"
    | "centre"
    | "north"
    | "top"
    | "northeast"
    | "right top"
    | "east"
    | "right"
    | "southeast"
    | "right bottom"
    | "south"
    | "bottom"
    | "southwest"
    | "left bottom"
    | "west"
    | "left"
    | "northwest"
    | "left top";

  export interface ResizeOptions {
    /** Default `"fill"`. */
    fit?: Fit;
    /** Default `"lanczos3"`. */
    filter?: ResizeFilter;
    /** The padding colour for `"contain"`. Default transparent. */
    background?: Colour;
    /** Never upscale. */
    withoutEnlargement?: boolean;
    /** Never downscale. */
    withoutReduction?: boolean;
    /** Which part `"cover"` keeps and where `"contain"` places the image. Default `"center"`. */
    position?: Position;
  }

  export type ImageFormat = "jpeg" | "png" | "webp" | "avif" | "gif" | "tiff" | "jxl";

  /** sharp's blend modes. */
  export type Blend =
    | "clear"
    | "source"
    | "over"
    | "in"
    | "out"
    | "atop"
    | "dest"
    | "dest-over"
    | "dest-in"
    | "dest-out"
    | "dest-atop"
    | "xor"
    | "add"
    | "saturate"
    | "multiply"
    | "screen"
    | "overlay"
    | "darken"
    | "lighten"
    | "colour-dodge"
    | "color-dodge"
    | "colour-burn"
    | "color-burn"
    | "hard-light"
    | "soft-light"
    | "difference"
    | "exclusion";

  export interface ExtendOptions {
    top?: number;
    right?: number;
    bottom?: number;
    left?: number;
    /** What fills the margin. Default `"background"`. */
    extendWith?: "background" | "copy" | "repeat" | "mirror";
    /** Default opaque black; a translucent colour adds alpha. */
    background?: Colour;
  }

  export interface TrimOptions {
    /** The border colour. Default: the top-left pixel's. */
    background?: Colour;
    /** How far a channel may differ and still be border. Default 10. */
    threshold?: number;
    /** Keep one-pixel lines (no median). Default false. */
    lineArt?: boolean;
  }

  /** One image for `composite`, drawn in array order, the first lowest. */
  export interface CompositeOverlay {
    /** An Image, or what `new Image` takes. */
    input: Image | ImageInput;
    /** Default `"over"`. */
    blend?: Blend;
    /** Where it sits unless `left` and `top` place it. Default `"center"`. */
    gravity?: Position;
    /** With `top`: the overlay's top-left corner. Give both or neither. */
    left?: number;
    top?: number;
    /** Repeat it across the image. Default false. */
    tile?: boolean;
  }

  export type ChromaSubsampling = "4:2:0" | "4:4:4";

  export interface PaletteOptions {
    /** Palette size, 2–256. */
    colours?: number;
    /** Bun's and sharp's other spelling of `colours`. */
    colors?: number;
    /** Error diffusion, 0–1; `true` is 1 and `false` 0. */
    dither?: number | boolean;
  }

  export type PixelFormat =
    | "gray8"
    | "gray16"
    | "graya8"
    | "rgb8"
    | "rgba8"
    | "rgb16"
    | "rgba16"
    | "rgbf32"
    | "rgbaf32";

  /**
   * `width`, `height`, `pixelFormat`, `channels`, `hasAlpha` and the trim
   * offsets describe the image at this point in the chain; the rest is what
   * the source file says about itself, absent when it says nothing.
   */
  export interface ImageMetadata {
    width: number;
    height: number;
    /** The source's format. */
    format: ImageFormat;
    pixelFormat: PixelFormat;
    channels: number;
    hasAlpha: boolean;
    /** What an animated file holds, or null for a still. */
    animation: { frames: number; loop: number; durations: number[] } | null;
    /** How many pages the source document holds. Always 1 except for TIFF. */
    pages: number;
    /** After a `trim`, minus the left and top of the box kept, as sharp reports them. */
    trimOffsetLeft?: number;
    trimOffsetTop?: number;
    /** The EXIF orientation, 1–8. */
    orientation?: number;
    /** Bits per stored sample, which may be fewer than `pixelFormat` holds. */
    bitsPerSample?: number;
    /** Pixels per inch. */
    density?: number;
    /** The unit the file states its density in. */
    resolutionUnit?: "inch" | "cm";
    isProgressive?: boolean;
    isPalette?: boolean;
    /** JPEG and AVIF. */
    chromaSubsampling?: string;
    /** The background colour the file names (PNG, GIF), in its own samples. */
    background?: { r: number; g: number; b: number } | { gray: number };
    /** The compression scheme where a format has several. */
    compression?: string;
    /** PNG text chunks and JPEG and GIF comments. */
    comments?: { keyword: string; text: string }[];
    hasProfile?: boolean;
    icc?: Uint8Array<ArrayBuffer>;
    exif?: Uint8Array<ArrayBuffer>;
    xmp?: Uint8Array<ArrayBuffer>;
  }

  /** One channel's statistics, in its own sample scale. */
  export interface ChannelStats {
    min: number;
    max: number;
    sum: number;
    squaresSum: number;
    mean: number;
    stdev: number;
    minX: number;
    minY: number;
    maxX: number;
    maxY: number;
  }

  export interface ImageStats {
    /** Each channel, alpha included. */
    channels: ChannelStats[];
    /** No alpha channel, or every pixel opaque. */
    isOpaque: boolean;
    /** Greyscale histogram entropy, in bits. */
    entropy: number;
    /** The standard deviation of the greyscale Laplacian. */
    sharpness: number;
    /** The most common sRGB colour. */
    dominant: { r: number; g: number; b: number };
  }

  /** The codes `runtime:images` rejects with. */
  export type ImageErrorCode = "ERR_IMAGE_DECODE_FAILED" | "ERR_IMAGE_FORMAT_UNSUPPORTED" | "ERR_IMAGE_TOO_MANY_PIXELS";

  /**
   * A lazy image pipeline. Every transform and format method returns a new
   * `Image`; nothing is decoded until `metadata()` or a terminal is awaited.
   */
  export class Image {
    constructor(input: ImageInput, options?: ImageOptions);

    /** Resize; omit either side (or pass null) to keep the aspect ratio. */
    resize(width: number | null, height?: number | null, options?: ResizeOptions): Image;
    /** Keep a region. `left` and `top` default to 0. */
    crop(region: { left?: number; top?: number; width: number; height: number }): Image;
    /** Rotate clockwise by a multiple of 90 degrees. */
    rotate(degrees: number): Image;
    /** Mirror top to bottom. */
    flip(): Image;
    /** Mirror left to right. */
    flop(): Image;
    /** Brightness and saturation multipliers, a hue rotation in degrees, and a lightness shift. */
    modulate(options: { brightness?: number; saturation?: number; hue?: number; lightness?: number }): Image;
    /** Gaussian blur of `sigma` pixels. */
    blur(sigma: number): Image;
    /** A 3×3 sharpen scaled by `amount` (default 1). */
    sharpen(amount?: number): Image;
    /** Draw over `background` (default black) and drop alpha. */
    flatten(options?: { background?: Colour }): Image;
    /** Grey, keeping alpha. */
    grayscale(): Image;
    /** Greyscale by luma, keeping the channel count. Unlike `grayscale`, RGB stays RGB. */
    greyscale(): Image;
    /** One channel as a grey image. */
    extractChannel(channel: 0 | 1 | 2 | 3 | "red" | "green" | "blue" | "alpha"): Image;
    /** Invert every channel; `alpha` (default true) inverts alpha too. */
    negate(options?: { alpha?: boolean }): Image;
    /** Gamma correction: each channel becomes `value^(1 / exponent)`. */
    gamma(exponent: number): Image;
    /** `value * a + b` on every channel; each side a number or three. */
    linear(a: number | [number, number, number], b?: number | [number, number, number]): Image;
    /** Two levels at `level` (default 128); `greyscale` (default true) thresholds luma. */
    threshold(level?: number, options?: { greyscale?: boolean }): Image;
    /** Tint towards a colour, keeping each pixel's luma. */
    tint(color: Colour): Image;
    /** Mix channels through a 3×3 or 4×4 matrix. */
    recomb(matrix: number[][]): Image;
    /** Add an alpha channel at `alpha` (default opaque) if there is none. */
    ensureAlpha(alpha?: number): Image;
    /** Drop the alpha channel without compositing. */
    removeAlpha(): Image;
    /** Grow the canvas by `n` pixels on every side, or by the margins given. */
    extend(extend: number | ExtendOptions): Image;
    /** Cut away a border; runs the chain before it in memory. */
    trim(options?: TrimOptions): Image;
    /** Draw images over this one, the first lowest. */
    composite(overlays: CompositeOverlay[]): Image;

    /** Copy the source's EXIF into the output. */
    keepExif(): Image;
    /** Copy the source's XMP into the output. */
    keepXmp(): Image;
    /** Copy the source's EXIF and XMP. */
    keepMetadata(): Image;
    /** Write this XMP packet in place of the source's. */
    withXmp(xmp: string): Image;
    /** Keep EXIF and XMP; `density` writes a resolution in DPI. */
    withMetadata(options?: { density?: number }): Image;

    jpeg(options?: { quality?: number; progressive?: boolean; chromaSubsampling?: ChromaSubsampling }): Image;
    png(options?: { compressionLevel?: number; palette?: boolean } & PaletteOptions): Image;
    webp(options?: {
      quality?: number;
      lossless?: boolean;
      nearLossless?: boolean;
      /** 0–6, default 4. */
      effort?: number;
      /** 0–100, default 100. */
      alphaQuality?: number;
    }): Image;
    avif(options?: {
      quality?: number;
      /** Default `"4:2:0"`. */
      chromaSubsampling?: ChromaSubsampling;
      /** 0–9, default 4. */
      effort?: number;
    }): Image;
    gif(options?: PaletteOptions): Image;
    tiff(): Image;
    jxl(options?: { quality?: number; lossless?: boolean }): Image;

    /** The image at this point in the chain, from its header. */
    metadata(): Promise<ImageMetadata>;
    /** Pixel statistics of the result; runs the chain. */
    stats(): Promise<ImageStats>;
    /** The encoded image; without a format method, in the source's format. */
    bytes(): Promise<Uint8Array<ArrayBuffer>>;
    /** The encoded image as a Blob of the format's media type. */
    blob(): Promise<Blob>;
    /** Encode into a `runtime:fs` file; resolves to the bytes written. */
    write(destination: FsFile): Promise<number>;
  }

  const images: { Image: typeof Image };
  export default images;
}
