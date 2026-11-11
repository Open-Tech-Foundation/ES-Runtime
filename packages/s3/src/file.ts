/**
 * An object, named by its key: read it, write it, sign a link to it.
 *
 * Making an `S3File` sends nothing; each method is one request (a write may be
 * several). The shape is Bun's `S3File` — `text()`, `json()`, `bytes()`,
 * `stream()`, `slice()`, `write()`, `writer()`, `presign()`, `exists()`,
 * `stat()`, `delete()` — except that `presign()` returns a promise, because
 * credentials may come from a provider that has to be asked first.
 */
import type { Core, Pairs } from "./core.js";
import { invalid } from "./errors.js";
import type { Acl, ReadOptions, WriteOptions } from "./options.js";
import { readHeaders } from "./options.js";
import { type Chunk, type S3WriteResult, S3Writer, writableOf } from "./writer.js";

export type S3FileOptions = ReadOptions & WriteOptions;

export interface S3Stats {
  size: number;
  lastModified: Date;
  /** Quoted, as S3 returns it — the form `ifMatch` takes. */
  etag: string;
  type: string;
  /** `x-amz-meta-*`, without the prefix. */
  metadata: Record<string, string>;
  versionId: string | undefined;
  storageClass: string | undefined;
  contentEncoding: string | undefined;
  contentDisposition: string | undefined;
  cacheControl: string | undefined;
}

export interface S3PresignOptions {
  /** Seconds the link stays valid: 1 to 604800 (7 days). Default 86400 (a day). */
  expiresIn?: number;
  method?: "GET" | "PUT" | "DELETE" | "HEAD" | "POST";
  /** For a PUT link: the `Content-Type` the uploader must send. */
  type?: string;
  /** For a PUT link: the ACL the object gets. */
  acl?: Acl;
  /** For a GET link: the `Content-Disposition` S3 answers with (`attachment; filename="…"`). */
  contentDisposition?: string;
  /** For a GET link: the `Content-Type` S3 answers with. */
  responseType?: string;
}

/** What an `S3File` needs from the client that made it. */
export interface FileHost {
  core: Core;
  write(bucket: string, key: string, data: unknown, options: WriteOptions): Promise<S3WriteResult>;
  presign(bucket: string, key: string, options: S3PresignOptions & ReadOptions): Promise<string>;
}

type Range = { start: number; end: number | undefined } | undefined;

export class S3File {
  readonly #host: FileHost;
  readonly #bucket: string;
  readonly #key: string;
  readonly #options: S3FileOptions;
  readonly #range: Range;

  /** @internal Made by `S3Client.file()`. */
  constructor(host: FileHost, bucket: string, key: string, options: S3FileOptions, range?: Range) {
    this.#host = host;
    this.#bucket = bucket;
    this.#key = key;
    this.#options = options;
    this.#range = range;
  }

  get key(): string {
    return this.#key;
  }

  /** The key — Bun's name for it. */
  get name(): string {
    return this.#key;
  }

  get bucket(): string {
    return this.#bucket;
  }

  /**
   * A view of part of the object, `Blob.slice` style: `start` inclusive, `end`
   * exclusive, a negative `start` counting from the end. Reading it sends a
   * `Range` request. A negative `end` needs the object's size, so it is
   * refused: `stat()` first and pass positive offsets.
   */
  slice(start = 0, end?: number): S3File {
    if (!Number.isInteger(start) || (end !== undefined && !Number.isInteger(end))) {
      throw invalid("slice offsets are whole numbers");
    }
    if (end !== undefined && end < 0)
      throw invalid("a negative end needs the object's size: stat() first");
    if (start < 0 && end !== undefined) throw invalid("a negative start takes no end");
    const base = this.#range;
    if (base !== undefined) {
      if (start < 0 || base.start < 0)
        throw invalid("a slice of a suffix slice needs the object's size");
      const from = base.start + start;
      const to =
        end === undefined
          ? base.end
          : Math.min(base.start + end, base.end ?? Number.POSITIVE_INFINITY);
      return new S3File(this.#host, this.#bucket, this.#key, this.#options, {
        start: from,
        end: to,
      });
    }
    return new S3File(this.#host, this.#bucket, this.#key, this.#options, { start, end });
  }

  #rangeHeader(): string | undefined | null {
    const range = this.#range;
    if (range === undefined) return undefined;
    if (range.start < 0) return `bytes=${range.start}`;
    if (range.end === undefined) return range.start === 0 ? undefined : `bytes=${range.start}-`;
    if (range.end <= range.start) return null; // empty: nothing to fetch
    return `bytes=${range.start}-${range.end - 1}`;
  }

  #query(): Pairs {
    return this.#options.versionId === undefined ? [] : [["versionId", this.#options.versionId]];
  }

  async #get(): Promise<Response> {
    const range = this.#rangeHeader();
    if (range === null) return new Response(new Uint8Array(0));
    const core = this.#host.core;
    const headers = readHeaders(this.#options, core.isHttps(this.#bucket));
    if (range !== undefined) headers.push(["range", range]);
    const response = await core.send({
      method: "GET",
      bucket: this.#bucket,
      key: this.#key,
      query: this.#query(),
      headers,
      signal: this.#options.signal,
      retry: this.#options.retry,
      // A range past the end reads as nothing, as Blob.slice does.
      accept: range === undefined ? [] : [416],
    });
    if (response.status === 416) {
      await response.body?.cancel();
      return new Response(new Uint8Array(0));
    }
    return response;
  }

  async bytes(): Promise<Uint8Array> {
    return new Uint8Array(await (await this.#get()).arrayBuffer());
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    return (await this.#get()).arrayBuffer();
  }

  async text(): Promise<string> {
    return new TextDecoder().decode(await this.bytes());
  }

  async json<T = unknown>(): Promise<T> {
    return JSON.parse(await this.text()) as T;
  }

  async blob(): Promise<Blob> {
    return (await this.#get()).blob();
  }

  /** The object's bytes as they arrive. The request is sent when the stream is first read. */
  stream(): ReadableStream<Uint8Array> {
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    return new ReadableStream<Uint8Array>({
      pull: async (controller) => {
        if (reader === undefined) {
          const body = (await this.#get()).body;
          if (body === null) {
            controller.close();
            return;
          }
          reader = body.getReader();
        }
        const { done, value } = await reader.read();
        if (done) controller.close();
        else controller.enqueue(value);
      },
      cancel: (reason) => reader?.cancel(reason),
    });
  }

  /** `false` for a missing object; any other failure throws. */
  async exists(): Promise<boolean> {
    try {
      await this.#head();
      return true;
    } catch (error) {
      if ((error as { status?: number }).status === 404) return false;
      throw error;
    }
  }

  async #head(): Promise<Response> {
    const core = this.#host.core;
    return core.send({
      method: "HEAD",
      bucket: this.#bucket,
      key: this.#key,
      query: this.#query(),
      headers: readHeaders(this.#options, core.isHttps(this.#bucket)),
      signal: this.#options.signal,
      retry: this.#options.retry,
    });
  }

  async stat(): Promise<S3Stats> {
    const response = await this.#head();
    const h = response.headers;
    const metadata: Record<string, string> = {};
    for (const [name, value] of h) {
      if (name.startsWith("x-amz-meta-")) metadata[name.slice("x-amz-meta-".length)] = value;
    }
    return {
      size: Number(h.get("content-length") ?? 0),
      lastModified: new Date(h.get("last-modified") ?? 0),
      etag: h.get("etag") ?? "",
      type: h.get("content-type") ?? "",
      metadata,
      versionId: h.get("x-amz-version-id") ?? undefined,
      storageClass: h.get("x-amz-storage-class") ?? undefined,
      contentEncoding: h.get("content-encoding") ?? undefined,
      contentDisposition: h.get("content-disposition") ?? undefined,
      cacheControl: h.get("cache-control") ?? undefined,
    };
  }

  /** Deletes the object (or the version named by `versionId`). Deleting a missing key succeeds, as S3 says. */
  async delete(): Promise<void> {
    const response = await this.#host.core.send({
      method: "DELETE",
      bucket: this.#bucket,
      key: this.#key,
      query: this.#query(),
      headers: this.#options.requestPayer ? [["x-amz-request-payer", "requester"]] : [],
      signal: this.#options.signal,
      retry: this.#options.retry,
    });
    await response.body?.cancel();
  }

  /**
   * Writes the object. A string, bytes or a `Blob` up to `partSize` is one PUT;
   * a larger `Blob`, a `ReadableStream`, a `Response`, a `Request` or another
   * `S3File` is streamed as a multipart upload.
   */
  write(
    data:
      | string
      | ArrayBuffer
      | ArrayBufferView
      | Blob
      | ReadableStream<Chunk>
      | Response
      | Request
      | S3File,
    options?: WriteOptions,
  ): Promise<S3WriteResult> {
    if (this.#range !== undefined) return Promise.reject(invalid("a slice cannot be written"));
    return this.#host.write(this.#bucket, this.#key, data, { ...this.#options, ...options });
  }

  /** A writer for an upload made of many writes. */
  writer(options?: WriteOptions): S3Writer {
    if (this.#range !== undefined) throw invalid("a slice cannot be written");
    return new S3Writer(this.#host.core, this.#bucket, this.#key, { ...this.#options, ...options });
  }

  /** {@link writer} as a `WritableStream`: `source.pipeTo(file.writable())`. */
  writable(options?: WriteOptions): WritableStream<Chunk> {
    return writableOf(this.writer(options));
  }

  /** A URL that does what `method` says to this object, without credentials, until it expires. */
  presign(options?: S3PresignOptions): Promise<string> {
    return this.#host.presign(this.#bucket, this.#key, { ...this.#options, ...options });
  }
}
