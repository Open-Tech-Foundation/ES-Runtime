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

  export interface ResizeOptions {
    /** Default `"fill"`. */
    fit?: Fit;
    /** Default `"lanczos3"`. */
    filter?: ResizeFilter;
    /** The padding colour for `"contain"`. Default transparent. */
    background?: Colour;
    /** Never upscale. */
    withoutEnlargement?: boolean;
  }

  export type ImageFormat = "jpeg" | "png" | "webp" | "avif" | "gif" | "tiff";

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

  export interface ImageMetadata {
    width: number;
    height: number;
    /** The source's format. */
    format: ImageFormat;
    pixelFormat: PixelFormat;
    hasAlpha: boolean;
    /** What an animated file holds, or null for a still. */
    animation: { frames: number; loop: number; durations: number[] } | null;
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
    /** Brightness and saturation multipliers, and a hue rotation in degrees. */
    modulate(options: { brightness?: number; saturation?: number; hue?: number }): Image;
    /** Gaussian blur of `sigma` pixels. */
    blur(sigma: number): Image;
    /** A 3×3 sharpen scaled by `amount` (default 1). */
    sharpen(amount?: number): Image;
    /** Draw over `background` (default black) and drop alpha. */
    flatten(options?: { background?: Colour }): Image;
    /** Grey, keeping alpha. */
    grayscale(): Image;
    /** One channel as a grey image. */
    extractChannel(channel: 0 | 1 | 2 | 3 | "red" | "green" | "blue" | "alpha"): Image;

    jpeg(options?: { quality?: number }): Image;
    png(): Image;
    webp(options?: { quality?: number; lossless?: boolean }): Image;
    avif(options?: { quality?: number }): Image;
    gif(): Image;
    tiff(): Image;

    /** The image at this point in the chain, from its header. */
    metadata(): Promise<ImageMetadata>;
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
