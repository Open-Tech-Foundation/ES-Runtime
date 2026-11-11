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
import { invalid, S3Error, S3ErrorCode } from "./errors.js";
import type { Acl, ReadOptions, WriteOptions } from "./options.js";
import { readHeaders, retryOf } from "./options.js";
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

  /**
   * One GET. `resume` is a range and an ETag for picking a read back up: the
   * caller's own conditions are replaced by `If-Match` on the ETag the read
   * started with, so every piece comes from the same object.
   */
  async #get(resume?: { range: string; etag: string }): Promise<Response> {
    const range = resume ? resume.range : this.#rangeHeader();
    if (range === null) return new Response(new Uint8Array(0));
    const core = this.#host.core;
    const options: ReadOptions = resume
      ? {
          ...this.#options,
          ifMatch: resume.etag,
          ifNoneMatch: undefined as never,
          ifModifiedSince: undefined as never,
          ifUnmodifiedSince: undefined as never,
        }
      : this.#options;
    const headers = readHeaders(options, core.isHttps(this.#bucket));
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
      accept: range === undefined || resume ? [] : [416],
    });
    if (response.status === 416) {
      await response.body?.cancel();
      return new Response(new Uint8Array(0));
    }
    return response;
  }

  /**
   * The object's bytes, resumed where they stopped if the connection fails
   * mid-body. `retry` before the body starts is the request's own; this is the
   * same budget again for the body.
   *
   * A resume asks for the rest with `Range` and `If-Match` on the ETag of the
   * first answer, so an object replaced in between fails the read
   * (`PreconditionFailed`) rather than splicing two versions together. A body
   * that ends short of its `Content-Length` without an error is resumed too.
   * An object served without an ETag is not resumed: nothing would prove the
   * rest belongs to it.
   */
  #body(onHeaders?: (headers: Headers) => void): ReadableStream<Uint8Array> {
    const what = `GET ${this.#bucket}/${this.#key}`;
    let budget = retryOf(this.#options.retry, this.#host.core.config.retry);
    let reader: ReadableStreamDefaultReader<Uint8Array> | undefined;
    let etag: string | undefined;
    /** The first byte of the read and the last (inclusive), in the object. */
    let first = 0;
    let last: number | undefined;
    let expected: number | undefined;
    let received = 0;

    const open = async (response: Response) => {
      reader = response.body?.getReader();
      if (reader === undefined) expected = 0;
    };
    const start = async () => {
      const response = await this.#get();
      onHeaders?.(response.headers);
      etag = response.headers.get("etag") ?? undefined;
      const span = /^bytes (\d+)-(\d+)\/(?:\d+|\*)$/.exec(
        response.headers.get("content-range") ?? "",
      );
      if (response.status === 206 && span) {
        first = Number(span[1]);
        last = Number(span[2]);
        expected = last - first + 1;
      } else {
        const length = response.headers.get("content-length");
        expected = length === null ? undefined : Number(length);
        if (expected !== undefined) last = expected - 1;
      }
      await open(response);
    };
    const resume = async (cause: unknown) => {
      if (this.#options.signal?.aborted) throw this.#options.signal.reason;
      if (budget <= 0 || etag === undefined) throw cause;
      budget--;
      await reader?.cancel().catch(() => {});
      const from = first + received;
      const range = `bytes=${from}-${last ?? ""}`;
      let response: Response;
      try {
        response = await this.#get({ range, etag });
      } catch (error) {
        if ((error as S3Error).code === "PreconditionFailed") {
          throw new S3Error(`${what}: the object changed while it was being read`, {
            code: "PreconditionFailed",
            status: 412,
            bucket: this.#bucket,
            key: this.#key,
            cause: error,
          });
        }
        throw error;
      }
      const span = /^bytes (\d+)-/.exec(response.headers.get("content-range") ?? "");
      if (response.status !== 206 || !span || Number(span[1]) !== from) {
        await response.body?.cancel();
        throw new S3Error(`${what}: the server did not resume at byte ${from}`, {
          code: S3ErrorCode.Response,
          bucket: this.#bucket,
          key: this.#key,
          cause,
        });
      }
      await open(response);
    };

    return new ReadableStream<Uint8Array>({
      pull: async (controller) => {
        if (reader === undefined && expected === undefined) await start();
        for (;;) {
          if (reader === undefined) {
            controller.close();
            return;
          }
          let chunk: ReadableStreamReadResult<Uint8Array>;
          try {
            chunk = await reader.read();
          } catch (cause) {
            await resume(cause);
            continue;
          }
          if (chunk.done) {
            if (expected !== undefined && received < expected) {
              await resume(
                new S3Error(`${what}: the body ended at ${received} of ${expected} bytes`, {
                  code: S3ErrorCode.Network,
                  bucket: this.#bucket,
                  key: this.#key,
                }),
              );
              continue;
            }
            controller.close();
            return;
          }
          received += chunk.value.length;
          controller.enqueue(chunk.value);
          return;
        }
      },
      cancel: (reason) => reader?.cancel(reason),
    });
  }

  async bytes(): Promise<Uint8Array> {
    return new Uint8Array(await new Response(this.#body()).arrayBuffer());
  }

  async arrayBuffer(): Promise<ArrayBuffer> {
    return new Response(this.#body()).arrayBuffer();
  }

  async text(): Promise<string> {
    return new TextDecoder().decode(await this.bytes());
  }

  async json<T = unknown>(): Promise<T> {
    return JSON.parse(await this.text()) as T;
  }

  async blob(): Promise<Blob> {
    let type = "";
    const bytes = await new Response(
      this.#body((headers) => {
        type = headers.get("content-type") ?? "";
      }),
    ).arrayBuffer();
    return new Blob([bytes], { type });
  }

  /**
   * The object's bytes as they arrive. The request is sent when the stream is
   * first read, and a connection lost mid-body is resumed (see {@link bytes}).
   */
  stream(): ReadableStream<Uint8Array> {
    return this.#body();
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
