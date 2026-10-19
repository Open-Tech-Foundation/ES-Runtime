declare module "runtime:archive" {
  import type { FsFile } from "runtime:fs";

  /** One entry's value when building: its bytes, a string, a Blob/File, or `null` for a directory. */
  export type ArchiveData = string | ArrayBuffer | ArrayBufferView | Blob;

  /** An entry that carries its own timestamp (epoch milliseconds or a Date). */
  export interface ArchiveEntryInput {
    data: ArchiveData | null;
    lastModified?: number | Date | null;
  }

  /**
   * A plain object keyed by entry path. A directory is `null` or
   * `{ data: null }` under a key ending in `/`.
   */
  export type ArchiveEntries = Record<string, ArchiveData | ArchiveEntryInput | null>;

  /** Where an archive is read from. A path string is refused: pass `file(path)`. */
  export type ArchiveInput = ArchiveEntries | ArrayBuffer | ArrayBufferView | Blob | FsFile;

  /** The formats an archive can be built in and read back. */
  export type ArchiveFormat = "tar" | "zip";

  export interface ArchiveOptions {
    /** The format to build. Default `"tar"`. Ignored when opening. */
    format?: ArchiveFormat;
    /** `"gzip"` with tar, `"deflate"` with zip, or `"none"`. Default `"none"`. */
    compress?: "none" | "gzip" | "deflate";
    /** Entry-count cap. Default 100000. */
    maxEntries?: number;
    /** Cap on uncompressed bytes. Default 1 GiB. */
    maxBytes?: number;
  }

  /** The read limits an entry read obeys. */
  export interface ArchiveLimits {
    maxEntries?: number;
    maxBytes?: number;
  }

  export interface ArchiveEntryMetadata {
    path: string;
    size: number;
    directory: boolean;
    /** Epoch milliseconds, or `null` when the archive carries no timestamp. */
    lastModified: number | null;
  }

  export interface ExtractResult {
    files: number;
    directories: number;
  }

  /** A predicate on an entry path, a path prefix, or a pattern. */
  export type ArchiveFilter = string | RegExp | ((path: string) => boolean);

  /** One entry, with metadata up front and contents read on demand. */
  export class ArchiveEntry {
    constructor(archive: Archive, metadata: ArchiveEntryMetadata);
    readonly path: string;
    readonly size: number;
    readonly directory: boolean;
    readonly lastModified: number | null;
    bytes(): Promise<Uint8Array>;
    text(): Promise<string>;
    blob(): Promise<Blob>;
    stream(): ReadableStream<Uint8Array>;
  }

  export class Archive {
    constructor(input: ArchiveInput, options?: ArchiveOptions);

    /** A copy of the archive's bytes. */
    bytes(): Promise<Uint8Array>;
    /** A Blob typed `application/x-tar`, `application/gzip` or `application/zip`. */
    blob(): Promise<Blob>;
    /** Every entry's metadata, in archive order. */
    entries(): Promise<ArchiveEntry[]>;
    /** The file entries as Files (directories are skipped). */
    files(filter?: ArchiveFilter): Promise<Map<string, File>>;
    /** Writes every entry under `directory`. Needs FileWrite. */
    extract(directory: string | FsFile, options?: ArchiveLimits): Promise<ExtractResult>;

    /** Writes an archive built from `input` to `destination`. Needs FileWrite. */
    static write(
      destination: string | FsFile,
      input: Archive | ArchiveInput,
      options?: ArchiveOptions,
    ): Promise<number>;
  }
}
