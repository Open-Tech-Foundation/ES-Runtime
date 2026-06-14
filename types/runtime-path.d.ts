declare module "runtime:path" {
  /** The parsed shape of a path, from {@link parse}. */
  export interface ParsedPath {
    /** The root of the path (`"/"`, `"C:\\"`, or `""`). */
    root: string;
    /** The directory portion. */
    dir: string;
    /** The final segment, including any extension. */
    base: string;
    /** The final segment without its extension. */
    name: string;
    /** The extension, including the leading dot (or `""`). */
    ext: string;
  }

  /** Path segment separator for the host OS (`"/"` or `"\\"`). */
  export const sep: string;

  /** Path list delimiter for the host OS (`":"` or `";"`). */
  export const delimiter: string;

  /** Whether `p` is an absolute path. */
  export function isAbsolute(p: string): boolean;

  /** Collapses `.`/`..` and redundant separators. */
  export function normalize(p: string): string;

  /** Joins segments with the separator, then normalizes. */
  export function join(...segments: string[]): string;

  /** Resolves to an absolute path, anchoring at `cwd()` if no segment is absolute. */
  export function resolve(...segments: string[]): string;

  /** The directory portion of `p`. */
  export function dirname(p: string): string;

  /** The final segment of `p`. */
  export function basename(p: string): string;

  /** The extension of the final segment, including the dot (or `""`). */
  export function extname(p: string): string;

  /** Splits `p` into `{ root, dir, base, name, ext }`. */
  export function parse(p: string): ParsedPath;

  /** Relative path from `from` to `to` (both resolved first). */
  export function relative(from: string, to: string): string;

  /** Converts a `file:` URL to a path. */
  export function fromFileURL(url: string | URL): string;

  /** Converts a path (resolved to absolute) to a `file:` URL. */
  export function toFileURL(p: string): URL;

  const path: {
    sep: typeof sep;
    delimiter: typeof delimiter;
    isAbsolute: typeof isAbsolute;
    normalize: typeof normalize;
    join: typeof join;
    resolve: typeof resolve;
    dirname: typeof dirname;
    basename: typeof basename;
    extname: typeof extname;
    parse: typeof parse;
    relative: typeof relative;
    fromFileURL: typeof fromFileURL;
    toFileURL: typeof toFileURL;
  };
  export default path;
}
