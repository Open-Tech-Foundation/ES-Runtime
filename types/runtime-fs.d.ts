declare module "runtime:fs" {
  /** A path: a string, a `file:` URL, or a {@link FsFile} handle. */
  export type PathLike = string | URL | FsFile;

  /** Anything `write` accepts as the body to write. */
  export type WriteInput =
    | string
    | Blob
    | ArrayBuffer
    | ArrayBufferView
    | Response
    | ReadableStream<Uint8Array>
    | FsFile;

  /** File metadata, from {@link stat} / {@link FsFile.stat} (follows symlinks). */
  export interface Stat {
    /** Size in bytes. */
    size: number;
    isFile: boolean;
    isDir: boolean;
    isSymlink: boolean;
    /** Modification time in ms since the Unix epoch, or `null` if unavailable. */
    mtimeMs: number | null;
  }

  /** One entry of a directory listing, from {@link readDir}. */
  export interface DirEntry {
    name: string;
    isFile: boolean;
    isDir: boolean;
    isSymlink: boolean;
  }

  /** Options for {@link Glob.scan}. */
  export interface GlobScanOptions {
    /** Directory to scan (default `"."`). */
    cwd?: string;
    /** Match dotfiles / dot-directories (default `false`). */
    dot?: boolean;
    /** Yield absolute paths instead of paths relative to `cwd` (default `false`). */
    absolute?: boolean;
    /** Yield only files, skipping directories (default `true`). */
    onlyFiles?: boolean;
    /** Traverse symlinked directories; entries leaving the root jail are skipped (default `false`). */
    followSymlinks?: boolean;
  }

  /**
   * A lazy, Blob-like file handle from {@link file}. Nothing is read until a
   * read method is called. **All methods are async.**
   */
  export interface FsFile {
    /** The path this handle points at. */
    readonly path: string;
    /** Read the whole file as text (UTF-8). */
    text(): Promise<string>;
    /** Read and `JSON.parse` the file. */
    json(): Promise<any>;
    /** Read the whole file as bytes. */
    bytes(): Promise<Uint8Array>;
    /** Read the whole file as an `ArrayBuffer`. */
    arrayBuffer(): Promise<ArrayBuffer>;
    /** A `ReadableStream` of the file's bytes. */
    stream(): ReadableStream<Uint8Array>;
    /** Whether the file exists. */
    exists(): Promise<boolean>;
    /** File metadata (follows symlinks). */
    stat(): Promise<Stat>;
    /** Write `data` to this file; resolves to bytes written. */
    write(data: WriteInput, options?: { append?: boolean }): Promise<number>;
    /** Delete this file. */
    delete(): Promise<void>;
    /**
     * A `WritableStream` sink for incremental / piped writes:
     * `await readable.pipeTo(file("out").writable({ append: true }))`.
     * The first chunk truncates the file unless { append: true } is provided;
     * later chunks append.
     */
    writable(options?: { append?: boolean }): WritableStream<Uint8Array | string>;
  }

  /**
   * Glob matching and scanning. Patterns support `*`, `**`, `?`, `[classes]`,
   * and `{a,b}` alternation; `*` does not cross `/`, `**` does.
   */
  export class Glob {
    constructor(pattern: string);
    /** Pure pattern match against a path (synchronous; no I/O). */
    match(path: PathLike): boolean;
    /** Walk the (jailed) filesystem, yielding matching paths. Needs `FileRead`. */
    scan(options?: string | GlobScanOptions): AsyncIterableIterator<string>;
  }

  /** A lazy, Blob-like handle for `path`. */
  export function file(path: PathLike): FsFile;

  /** Write any web body to `dest`; resolves to bytes written. Needs `FileWrite`. */
  export function write(
    dest: PathLike,
    input: WriteInput,
    options?: { append?: boolean },
  ): Promise<number>;

  /** List the entries of a directory. Needs `FileRead`. */
  export function readDir(path: PathLike): Promise<DirEntry[]>;

  /** Metadata for `path` (follows symlinks). Needs `FileRead`. */
  export function stat(path: PathLike): Promise<Stat>;

  /** Whether `path` exists (missing → `false`). Needs `FileRead`. */
  export function exists(path: PathLike): Promise<boolean>;

  /** Create a directory (`recursive` creates parents). Needs `FileWrite`. */
  export function mkdir(path: PathLike, options?: { recursive?: boolean }): Promise<void>;

  /** Remove a file or (with `recursive`) a directory tree. Needs `FileWrite`. */
  export function remove(path: PathLike, options?: { recursive?: boolean }): Promise<void>;

  /** Rename/move an entry (both jailed). Needs `FileWrite`. */
  export function rename(from: PathLike, to: PathLike): Promise<void>;

  /**
   * Copy a file, overwriting `to`; resolves to bytes copied. Needs **both**
   * `FileRead` and `FileWrite` — it reads one path and writes another, and
   * gating it on the write alone would let a caller duplicate a file it cannot
   * read into somewhere it can reach another way.
   *
   * Rejects with `ERR_SAME_FILE` if `from` and `to` are the same file
   * (including two hardlinks to one inode), which would otherwise truncate it.
   */
  export function copy(from: PathLike, to: PathLike): Promise<number>;

  /**
   * The canonical location of `path`: symlinks followed, `.`/`..` removed.
   * Rejects if it does not exist (`ERR_NOT_FOUND`) or resolves outside the root
   * jail (`ERR_JAIL_ESCAPE`). Needs `FileRead`.
   */
  export function realPath(path: PathLike): Promise<string>;

  /**
   * The stored target of a symbolic link, **verbatim** — it may be relative and
   * may not exist. Use {@link realPath} to resolve it. Needs `FileRead`.
   */
  export function readLink(path: PathLike): Promise<string>;

  /** Set the file's length exactly, zero-filling if it grows. Needs `FileWrite`. */
  export function truncate(path: PathLike, len?: number): Promise<void>;

  /**
   * Set permission bits — a Unix mode such as `0o600`. Needs `FileWrite`.
   *
   * Windows has no mode bits: only the owner-write bit is honoured, as the
   * read-only flag.
   */
  export function chmod(path: PathLike, mode: number): Promise<void>;

  /** Options for {@link makeTempDir} / {@link makeTempFile}. */
  export interface TempOptions {
    /**
     * Where to create it. Defaults to the base directory — **not** the OS temp
     * directory, which is outside the root jail.
     */
    dir?: PathLike;
    /** Prefix for the generated name. */
    prefix?: string;
  }

  /**
   * Create a directory with an unpredictable name and return its path. Nothing
   * is cleaned up automatically — the caller owns it. Needs `FileWrite`.
   */
  export function makeTempDir(options?: TempOptions): Promise<string>;

  /** Create an empty file with an unpredictable name and return its path. Needs `FileWrite`. */
  export function makeTempFile(options?: TempOptions): Promise<string>;

  const fs: {
    file: typeof file;
    write: typeof write;
    readDir: typeof readDir;
    stat: typeof stat;
    exists: typeof exists;
    mkdir: typeof mkdir;
    remove: typeof remove;
    rename: typeof rename;
    copy: typeof copy;
    realPath: typeof realPath;
    readLink: typeof readLink;
    truncate: typeof truncate;
    chmod: typeof chmod;
    makeTempDir: typeof makeTempDir;
    makeTempFile: typeof makeTempFile;
    Glob: typeof Glob;
  };
  export default fs;
}
