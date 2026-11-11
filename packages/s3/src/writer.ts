/**
 * Uploads of any size: buffered into parts, a few in flight at once.
 *
 * Memory is bounded by `partSize × (queueSize + 1)`: the part being filled and
 * the ones being sent. A `write` waits while the queue is full, which is the
 * backpressure that keeps a fast source from outrunning the network.
 *
 * Nothing is created on the server until the first part is full. A writer that
 * ends before then sends one ordinary PUT, so a small upload costs one request,
 * not three. A part that fails after its retries fails the upload: the
 * multipart upload is aborted, so no orphaned parts are left to be billed.
 */
import type { Core } from "./core.js";
import { invalid, S3Error, S3ErrorCode } from "./errors.js";
import {
  checksumName,
  checksumValue,
  conditionalWriteHeaders,
  MAX_PARTS,
  objectHeaders,
  partSizeOf,
  queueSizeOf,
  retryOf,
  sseCustomerHeaders,
  type WriteOptions,
} from "./options.js";
import { build, childText, NAMESPACE, parse } from "./xml.js";

export interface S3WriteResult {
  /** The new object's ETag, quoted as S3 returns it. */
  etag: string;
  /** The new version, in a versioned bucket. */
  versionId: string | undefined;
  /** Bytes written. */
  size: number;
}

const encoder = new TextEncoder();

export type Chunk = string | ArrayBuffer | ArrayBufferView;

/** A copy: the caller may reuse its buffer as soon as `write` returns. */
export function chunkBytes(chunk: Chunk): Uint8Array {
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk.slice(0));
  if (ArrayBuffer.isView(chunk)) {
    return new Uint8Array(
      chunk.buffer.slice(chunk.byteOffset, chunk.byteOffset + chunk.byteLength),
    );
  }
  throw invalid("a chunk is a string, an ArrayBuffer or a typed array");
}

interface Part {
  PartNumber: number;
  ETag: string;
  checksum?: string;
}

/** One PUT of a body already in memory. */
export async function putObject(
  core: Core,
  bucket: string,
  key: string,
  body: Uint8Array,
  options: WriteOptions,
  fallbackType: string | undefined,
): Promise<S3WriteResult> {
  const headers = [
    ...objectHeaders(key, options, fallbackType),
    ...conditionalWriteHeaders(options),
    ...sseCustomerHeaders(options.sseCustomerKey, core.isHttps(bucket)),
  ];
  const name = checksumName(options.checksum);
  if (name !== undefined)
    headers.push([`x-amz-checksum-${options.checksum}`, checksumValue(options.checksum!, body)]);
  const response = await core.send({
    method: "PUT",
    bucket,
    key,
    headers,
    body,
    signal: options.signal,
    retry: options.retry,
  });
  await response.body?.cancel();
  return {
    etag: response.headers.get("etag") ?? "",
    versionId: response.headers.get("x-amz-version-id") ?? undefined,
    size: body.length,
  };
}

export class S3Writer {
  readonly #core: Core;
  readonly #bucket: string;
  readonly #key: string;
  readonly #options: WriteOptions;
  readonly #fallbackType: string | undefined;
  readonly #partSize: number;
  readonly #queueSize: number;
  readonly #retry: number;

  #chunks: Uint8Array[] = [];
  #buffered = 0;
  #size = 0;
  #nextPart = 1;
  #upload: Promise<string> | undefined;
  #parts: Part[] = [];
  #inflight = new Set<Promise<void>>();
  #failure: unknown;
  /** The one abort of the multipart upload, however many paths ask for it. */
  #aborting: Promise<void> | undefined;
  #state: "open" | "ending" | "done" | "aborted" = "open";

  /** @internal Made by `S3File.writer()`. */
  constructor(
    core: Core,
    bucket: string,
    key: string,
    options: WriteOptions,
    fallbackType?: string,
  ) {
    this.#core = core;
    this.#bucket = bucket;
    this.#key = key;
    this.#options = options;
    this.#fallbackType = fallbackType;
    this.#partSize = partSizeOf(options.partSize);
    this.#queueSize = queueSizeOf(options.queueSize);
    this.#retry = retryOf(options.retry, core.config.retry);
    checksumName(options.checksum);
    // Refuse an SSE-C key on plain HTTP now, not after the first part.
    sseCustomerHeaders(options.sseCustomerKey, core.isHttps(bucket));
  }

  /** Bytes accepted so far. */
  get size(): number {
    return this.#size;
  }

  /** Adds `chunk`. Resolves once it is buffered — waiting, while the queue is full, for a part to finish. */
  async write(chunk: Chunk): Promise<void> {
    await this.#check();
    const bytes = chunkBytes(chunk);
    if (bytes.length === 0) return;
    this.#chunks.push(bytes);
    this.#buffered += bytes.length;
    this.#size += bytes.length;
    try {
      while (this.#buffered >= this.#partSize) {
        await this.#queue(this.#take(this.#partSize));
      }
    } catch (error) {
      throw await this.#fail(error);
    }
  }

  /** Sends what is left and completes the object. */
  async end(): Promise<S3WriteResult> {
    await this.#check();
    this.#state = "ending";
    try {
      if (this.#upload === undefined) {
        const result = await putObject(
          this.#core,
          this.#bucket,
          this.#key,
          this.#take(this.#buffered),
          { ...this.#options, retry: this.#retry },
          this.#fallbackType,
        );
        this.#state = "done";
        return result;
      }
      if (this.#buffered > 0) await this.#queue(this.#take(this.#buffered));
      await Promise.all(this.#inflight);
      if (this.#failure !== undefined) throw this.#failure;
      const result = await this.#complete(await this.#upload);
      this.#state = "done";
      return result;
    } catch (error) {
      throw await this.#fail(error);
    }
  }

  /** Abandons the upload, deleting any parts already sent. */
  async abort(reason?: unknown): Promise<void> {
    if (this.#state === "done") return;
    if (this.#state === "aborted") {
      // Failed already: resolve once that failure's cleanup has finished.
      await this.#aborting;
      return;
    }
    this.#state = "aborted";
    this.#failure ??=
      reason ??
      new S3Error(`the upload of ${this.#key} was aborted`, {
        code: S3ErrorCode.Aborted,
        key: this.#key,
      });
    await this.#abortOnce();
  }

  /**
   * Records `error` as the upload's failure and aborts the multipart upload,
   * resolving with the failure once nothing is left on the server. Every path
   * that fails comes here, so a caller that stops at the first rejected
   * `write()` leaves no parts behind, whether or not it calls `abort()`.
   */
  async #fail(error: unknown): Promise<unknown> {
    this.#failure ??= error;
    this.#state = "aborted";
    await this.#abortOnce();
    return this.#failure;
  }

  #abortOnce(): Promise<void> {
    this.#aborting ??= (async () => {
      // Parts still in flight finish (or fail) first: a part that lands after
      // the abort would be stored again under an upload nobody completes.
      await Promise.allSettled(this.#inflight);
      await this.#abortUpload();
    })();
    return this.#aborting;
  }

  async #check(): Promise<void> {
    if (this.#failure !== undefined) throw await this.#fail(this.#failure);
    if (this.#state !== "open") throw invalid(`the writer for ${this.#key} has ended`);
  }

  #take(length: number): Uint8Array {
    const out = new Uint8Array(length);
    let at = 0;
    while (at < length) {
      const head = this.#chunks[0]!;
      const need = length - at;
      if (head.length <= need) {
        out.set(head, at);
        at += head.length;
        this.#chunks.shift();
      } else {
        out.set(head.subarray(0, need), at);
        this.#chunks[0] = head.subarray(need);
        at = length;
      }
    }
    this.#buffered -= length;
    return out;
  }

  async #queue(part: Uint8Array): Promise<void> {
    while (this.#inflight.size >= this.#queueSize) await Promise.race(this.#inflight);
    if (this.#failure !== undefined) throw this.#failure;
    const number = this.#nextPart++;
    if (number > MAX_PARTS) {
      throw invalid(
        `${this.#key} needs more than ${MAX_PARTS} parts of ${this.#partSize} bytes; raise partSize`,
      );
    }
    this.#upload ??= this.#create();
    const task: Promise<void> = this.#uploadPart(number, part).then(
      (done) => {
        this.#parts[number - 1] = done;
      },
      (error) => {
        // Abort now rather than when the caller next writes: a caller that
        // never does would leave the upload open.
        void this.#fail(error);
      },
    );
    const tracked = task.finally(() => this.#inflight.delete(tracked));
    this.#inflight.add(tracked);
  }

  async #create(): Promise<string> {
    const headers = [
      ...objectHeaders(this.#key, this.#options, this.#fallbackType),
      ...sseCustomerHeaders(this.#options.sseCustomerKey, this.#core.isHttps(this.#bucket)),
    ];
    const algorithm = checksumName(this.#options.checksum);
    if (algorithm !== undefined) headers.push(["x-amz-checksum-algorithm", algorithm]);
    const response = await this.#core.send({
      method: "POST",
      bucket: this.#bucket,
      key: this.#key,
      query: [["uploads", ""]],
      headers,
      signal: this.#options.signal,
      retry: this.#retry,
    });
    const root = parse(await response.text(), `CreateMultipartUpload ${this.#key}`);
    const id = childText(root.InitiateMultipartUploadResult, "UploadId");
    if (!id) {
      throw new S3Error("CreateMultipartUpload returned no UploadId", {
        code: S3ErrorCode.Response,
        key: this.#key,
      });
    }
    return id;
  }

  async #uploadPart(number: number, body: Uint8Array): Promise<Part> {
    const uploadId = await this.#upload!;
    const headers = sseCustomerHeaders(
      this.#options.sseCustomerKey,
      this.#core.isHttps(this.#bucket),
    );
    let checksum: string | undefined;
    if (this.#options.checksum !== undefined) {
      checksum = checksumValue(this.#options.checksum, body);
      headers.push([`x-amz-checksum-${this.#options.checksum}`, checksum]);
    }
    const response = await this.#core.send({
      method: "PUT",
      bucket: this.#bucket,
      key: this.#key,
      query: [
        ["partNumber", String(number)],
        ["uploadId", uploadId],
      ],
      headers,
      body,
      signal: this.#options.signal,
      retry: this.#retry,
    });
    await response.body?.cancel();
    const etag = response.headers.get("etag");
    if (!etag) {
      throw new S3Error(`UploadPart ${number} returned no ETag`, {
        code: S3ErrorCode.Response,
        key: this.#key,
      });
    }
    return checksum === undefined
      ? { PartNumber: number, ETag: etag }
      : { PartNumber: number, ETag: etag, checksum };
  }

  async #complete(uploadId: string): Promise<S3WriteResult> {
    const checksum = checksumName(this.#options.checksum);
    const parts = this.#parts.map((part) => {
      const entry: Record<string, unknown> = { PartNumber: part.PartNumber, ETag: part.ETag };
      if (checksum !== undefined && part.checksum !== undefined)
        entry[`Checksum${checksum}`] = part.checksum;
      return entry;
    });
    const body = encoder.encode(
      build({ CompleteMultipartUpload: { "@xmlns": NAMESPACE, Part: parts } }),
    );
    const response = await this.#core.send({
      method: "POST",
      bucket: this.#bucket,
      key: this.#key,
      query: [["uploadId", uploadId]],
      headers: [
        ["content-type", "application/xml"],
        ...conditionalWriteHeaders(this.#options),
        ...sseCustomerHeaders(this.#options.sseCustomerKey, this.#core.isHttps(this.#bucket)),
      ],
      body,
      signal: this.#options.signal,
      retry: this.#retry,
      errorIn200: true,
    });
    const root = parse(await response.text(), `CompleteMultipartUpload ${this.#key}`);
    return {
      etag: childText(root.CompleteMultipartUploadResult, "ETag") ?? "",
      versionId: response.headers.get("x-amz-version-id") ?? undefined,
      size: this.#size,
    };
  }

  async #abortUpload(): Promise<void> {
    const pending = this.#upload;
    if (pending === undefined) return;
    this.#upload = undefined;
    let uploadId: string;
    try {
      uploadId = await pending;
    } catch {
      return; // never created, so nothing to abort
    }
    try {
      const response = await this.#core.send({
        method: "DELETE",
        bucket: this.#bucket,
        key: this.#key,
        query: [["uploadId", uploadId]],
        retry: this.#retry,
        accept: [404],
      });
      await response.body?.cancel();
    } catch {
      // Best effort: the original failure is what the caller needs to see. An
      // abandoned upload is what a bucket lifecycle rule
      // (AbortIncompleteMultipartUpload) is for.
    }
  }
}

/** A `WritableStream` over an {@link S3Writer}, for `pipeTo`. */
export function writableOf(writer: S3Writer): WritableStream<Chunk> {
  return new WritableStream<Chunk>({
    write: (chunk) => writer.write(chunk),
    close: async () => {
      await writer.end();
    },
    abort: (reason) => writer.abort(reason),
  });
}
