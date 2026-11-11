/**
 * `S3Client`: a bucket's address, credentials and defaults, and the operations
 * that are not about one object.
 *
 * Bun's `S3Client` is the model — the same constructor options, `file()`,
 * `write()`, `presign()`, `exists()`, `stat()`, `delete()`, `size()` and
 * `list()` — with three changes. The environment is read only when asked, by
 * {@link S3Client.fromEnv}; `presign()` returns a promise; `write()` resolves
 * with the ETag and version rather than a byte count, because those are what a
 * conditional write or a cache needs next.
 */
import { hash } from "runtime:hashing";
import { Core, checkBucket, checkKey, type Pairs } from "./core.js";
import {
  CredentialCache,
  type CredentialsProvider,
  type S3Credentials,
  type SecretValue,
} from "./credentials.js";
import { invalid, S3Error, S3ErrorCode } from "./errors.js";
import {
  type FileHost,
  S3File,
  type S3FileOptions,
  type S3PresignOptions,
  type S3Stats,
} from "./file.js";
import {
  conditionalWriteHeaders,
  objectHeaders,
  partSizeOf,
  queueSizeOf,
  type ReadOptions,
  retryOf,
  sseCustomerHeaders,
  typeOf,
  type WriteOptions,
} from "./options.js";
import { uriEncode, uriEncodePath } from "./sigv4.js";
import { type Chunk, putObject, type S3WriteResult, S3Writer } from "./writer.js";
import {
  build,
  child,
  childBoolean,
  childDate,
  childNumber,
  children,
  childText,
  NAMESPACE,
  parse,
} from "./xml.js";

export interface S3ClientOptions extends WriteOptions {
  accessKeyId?: SecretValue;
  secretAccessKey?: SecretValue;
  sessionToken?: SecretValue;
  /** Credentials, or a function that returns them — for temporary credentials that expire. */
  credentials?: S3Credentials | CredentialsProvider;
  /** The default bucket; any call can name another. */
  bucket?: string;
  /** Default `us-east-1`. Cloudflare R2 takes `auto`. */
  region?: string;
  /**
   * The service's URL: `https://<account>.r2.cloudflarestorage.com`,
   * `https://storage.googleapis.com`, `http://localhost:9000`. Default AWS's
   * regional endpoint.
   */
  endpoint?: string;
  /**
   * `false` (the default): path-style, `endpoint/bucket/key`. `true`: the
   * bucket is in the host — `endpoint` is then the bucket's own URL
   * (`https://my-bucket.s3.us-east-1.amazonaws.com`) and `bucket` is not added
   * to it, as Bun does.
   */
  virtualHostedStyle?: boolean;
}

export interface S3ListOptions {
  prefix?: string;
  /** Group keys sharing a prefix up to this string into `commonPrefixes` (`/` lists one "directory"). */
  delimiter?: string;
  /** At most this many keys per page, 1 to 1000. */
  maxKeys?: number;
  startAfter?: string;
  continuationToken?: string;
  fetchOwner?: boolean;
}

export interface S3ListObject {
  key: string;
  size: number;
  lastModified: Date | undefined;
  /** Quoted, as S3 returns it. */
  eTag: string | undefined;
  storageClass: string | undefined;
  checksumAlgorithm: string | undefined;
  owner: { id: string | undefined; displayName: string | undefined } | undefined;
}

export interface S3ListResult {
  name: string | undefined;
  prefix: string | undefined;
  delimiter: string | undefined;
  maxKeys: number | undefined;
  keyCount: number | undefined;
  isTruncated: boolean;
  continuationToken: string | undefined;
  /** Pass as `continuationToken` for the next page. */
  nextContinuationToken: string | undefined;
  startAfter: string | undefined;
  contents: S3ListObject[];
  commonPrefixes: Array<{ prefix: string }>;
}

export interface S3CallOptions {
  bucket?: string;
  signal?: AbortSignal;
}

export interface S3CopyOptions extends WriteOptions {
  bucket?: string;
  /** The bucket to copy from. Default: the destination's. */
  sourceBucket?: string;
  sourceVersionId?: string;
  /** Copy only if the source's ETag matches. */
  sourceIfMatch?: string;
  /** Replace the metadata rather than copying it. Implied by `metadata` or `type`. */
  replaceMetadata?: boolean;
}

export interface S3CopyResult {
  etag: string;
  lastModified: Date | undefined;
  versionId: string | undefined;
}

export interface S3DeleteError {
  key: string;
  versionId: string | undefined;
  code: string;
  message: string;
}

const encoder = new TextEncoder();
const REGION = /^[a-z0-9-]{1,64}$/;

function endpointOf(value: string | undefined): URL | undefined {
  if (value === undefined || value === "") return undefined;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw invalid(`endpoint "${value}" is not a URL`);
  }
  if (url.protocol !== "https:" && url.protocol !== "http:")
    throw invalid("endpoint is an http: or https: URL");
  if (url.username || url.password)
    throw invalid("endpoint must not carry credentials; pass them as options");
  if (url.search || url.hash) throw invalid("endpoint takes no query or fragment");
  return url;
}

/** `encoding-type=url` keys: `+` for a space, `%XX` for the rest (as botocore reads them). */
function decodeKey(value: string | undefined, encoded: boolean): string | undefined {
  if (value === undefined || !encoded) return value;
  try {
    return decodeURIComponent(value.replace(/\+/g, "%20"));
  } catch {
    throw new S3Error(`list returned a key that is not URL-encoded: ${value}`, {
      code: S3ErrorCode.Response,
    });
  }
}

export class S3Client {
  readonly #core: Core;
  readonly #bucket: string | undefined;
  readonly #defaults: WriteOptions;
  readonly #host: FileHost;

  constructor(options: S3ClientOptions = {}) {
    const {
      accessKeyId,
      secretAccessKey,
      sessionToken,
      credentials,
      bucket,
      region = "us-east-1",
      endpoint,
      virtualHostedStyle = false,
      ...defaults
    } = options;
    if (!REGION.test(region)) throw invalid(`region "${region}" is not a region name`);
    if (bucket !== undefined) checkBucket(bucket);
    partSizeOf(defaults.partSize);
    queueSizeOf(defaults.queueSize);
    const retry = retryOf(defaults.retry, 3);

    let provider: CredentialsProvider;
    if (typeof credentials === "function") {
      provider = credentials;
    } else if (credentials !== undefined) {
      const given = credentials;
      provider = () => given;
    } else if (accessKeyId !== undefined || secretAccessKey !== undefined) {
      provider = () => ({
        accessKeyId: accessKeyId!,
        secretAccessKey: secretAccessKey!,
        sessionToken,
      });
    } else {
      provider = () => {
        throw new S3Error(
          "no credentials: pass accessKeyId and secretAccessKey, a credentials provider, or use S3Client.fromEnv()",
          { code: S3ErrorCode.Credentials },
        );
      };
    }

    this.#core = new Core({
      endpoint: endpointOf(endpoint),
      region,
      virtualHostedStyle,
      retry,
      credentials: new CredentialCache(provider),
    });
    this.#bucket = bucket;
    this.#defaults = defaults;
    this.#host = {
      core: this.#core,
      write: (b, key, data, opts) => this.#write(b, key, data, opts),
      presign: (b, key, opts) => this.#presign(b, key, opts),
    };
  }

  /**
   * A client configured from the environment, as Bun's default `s3` is:
   * `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`, `S3_SESSION_TOKEN`, `S3_REGION`,
   * `S3_ENDPOINT` and `S3_BUCKET`, each falling back to its `AWS_*` name
   * (`AWS_ACCESS_KEY_ID`, …, `AWS_ENDPOINT_URL_S3`, `AWS_ENDPOINT_URL`,
   * `AWS_ENDPOINT`, `AWS_BUCKET`). `options` override what the environment
   * says. Needs `--allow-env` for the names it should read; a name not granted
   * reads as unset.
   */
  static async fromEnv(options: S3ClientOptions = {}): Promise<S3Client> {
    let env: Record<string, unknown>;
    try {
      env = (await import("runtime:process")).env as Record<string, unknown>;
    } catch (cause) {
      throw new S3Error("S3Client.fromEnv() needs the env capability (--allow-env=S3_BUCKET,…)", {
        code: S3ErrorCode.Credentials,
        cause,
      });
    }
    const first = (...names: string[]) => {
      for (const name of names) {
        const value = env[name];
        if (value !== undefined && value !== "") return value;
      }
      return undefined;
    };
    const plain = (...names: string[]) => {
      const value = first(...names);
      return value === undefined ? undefined : String(value);
    };
    const fromEnv: S3ClientOptions = {};
    const accessKeyId = first("S3_ACCESS_KEY_ID", "AWS_ACCESS_KEY_ID") as SecretValue | undefined;
    const secretAccessKey = first("S3_SECRET_ACCESS_KEY", "AWS_SECRET_ACCESS_KEY") as
      | SecretValue
      | undefined;
    const sessionToken = first("S3_SESSION_TOKEN", "AWS_SESSION_TOKEN") as SecretValue | undefined;
    if (accessKeyId !== undefined) fromEnv.accessKeyId = accessKeyId;
    if (secretAccessKey !== undefined) fromEnv.secretAccessKey = secretAccessKey;
    if (sessionToken !== undefined) fromEnv.sessionToken = sessionToken;
    const region = plain("S3_REGION", "AWS_REGION");
    if (region !== undefined) fromEnv.region = region;
    const endpoint = plain(
      "S3_ENDPOINT",
      "AWS_ENDPOINT_URL_S3",
      "AWS_ENDPOINT_URL",
      "AWS_ENDPOINT",
    );
    if (endpoint !== undefined) fromEnv.endpoint = endpoint;
    const bucket = plain("S3_BUCKET", "AWS_BUCKET");
    if (bucket !== undefined) fromEnv.bucket = bucket;
    return new S3Client({ ...fromEnv, ...options });
  }

  #bucketOf(bucket: string | undefined): string {
    return checkBucket(bucket ?? this.#bucket);
  }

  /** The object at `key`. Sends nothing until a method is called on it. */
  file(key: string, options: S3FileOptions & { bucket?: string } = {}): S3File {
    const { bucket, ...rest } = options;
    return new S3File(this.#host, this.#bucketOf(bucket), checkKey(key), {
      ...this.#defaults,
      ...rest,
    });
  }

  /** Writes `data` to `key`. See {@link S3File.write}. */
  write(
    key: string,
    data: Parameters<S3File["write"]>[0],
    options: WriteOptions & { bucket?: string } = {},
  ): Promise<S3WriteResult> {
    return this.file(key, options).write(data);
  }

  /** A writer for a multipart upload to `key`. */
  writer(key: string, options: WriteOptions & { bucket?: string } = {}): S3Writer {
    return this.file(key, options).writer();
  }

  delete(key: string, options: S3CallOptions & ReadOptions = {}): Promise<void> {
    return this.file(key, options).delete();
  }

  exists(key: string, options: S3CallOptions & ReadOptions = {}): Promise<boolean> {
    return this.file(key, options).exists();
  }

  stat(key: string, options: S3CallOptions & ReadOptions = {}): Promise<S3Stats> {
    return this.file(key, options).stat();
  }

  async size(key: string, options: S3CallOptions & ReadOptions = {}): Promise<number> {
    return (await this.stat(key, options)).size;
  }

  presign(
    key: string,
    options: S3PresignOptions & S3CallOptions & ReadOptions = {},
  ): Promise<string> {
    const { bucket, signal, ...rest } = options;
    return this.file(key, bucket === undefined ? {} : { bucket }).presign(rest);
  }

  async #write(
    bucket: string,
    key: string,
    data: unknown,
    options: WriteOptions,
  ): Promise<S3WriteResult> {
    const core = this.#core;
    const partSize = partSizeOf(options.partSize);
    if (typeof data === "string") {
      // The key's extension says more than "it was a string" does.
      return putObject(
        core,
        bucket,
        key,
        encoder.encode(data),
        options,
        typeOf(key) ?? "text/plain;charset=utf-8",
      );
    }
    if (data instanceof ArrayBuffer)
      return putObject(core, bucket, key, new Uint8Array(data), options, undefined);
    if (ArrayBuffer.isView(data)) {
      return putObject(
        core,
        bucket,
        key,
        new Uint8Array(data.buffer, data.byteOffset, data.byteLength),
        options,
        undefined,
      );
    }
    if (data instanceof Blob) {
      const type = data.type || undefined;
      if (data.size <= partSize) {
        return putObject(
          core,
          bucket,
          key,
          new Uint8Array(await data.arrayBuffer()),
          options,
          type,
        );
      }
      return this.#stream(bucket, key, data.stream(), options, type);
    }
    if (data instanceof Response || data instanceof Request) {
      const type = data.headers.get("content-type") ?? undefined;
      if (data.body === null) return putObject(core, bucket, key, new Uint8Array(0), options, type);
      return this.#stream(bucket, key, data.body, options, type);
    }
    if (data instanceof S3File) return this.#stream(bucket, key, data.stream(), options, undefined);
    if (data instanceof ReadableStream)
      return this.#stream(bucket, key, data as ReadableStream<Chunk>, options, undefined);
    throw invalid(
      "write takes a string, bytes, a Blob, a ReadableStream, a Response, a Request or an S3File",
    );
  }

  async #stream(
    bucket: string,
    key: string,
    stream: ReadableStream<Chunk>,
    options: WriteOptions,
    type: string | undefined,
  ): Promise<S3WriteResult> {
    const writer = new S3Writer(this.#core, bucket, key, options, type);
    const reader = stream.getReader();
    try {
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        await writer.write(value);
      }
    } catch (error) {
      await writer.abort(error);
      await reader.cancel(error).catch(() => {});
      throw error;
    }
    return writer.end();
  }

  async #presign(
    bucket: string,
    key: string,
    options: S3PresignOptions & ReadOptions,
  ): Promise<string> {
    const expiresIn = options.expiresIn ?? 86_400;
    if (!Number.isInteger(expiresIn) || expiresIn < 1 || expiresIn > 604_800) {
      throw invalid("expiresIn is a whole number of seconds from 1 to 604800 (7 days)");
    }
    const method = options.method ?? "GET";
    if (!["GET", "PUT", "DELETE", "HEAD", "POST"].includes(method))
      throw invalid(`cannot presign ${method}`);
    const query: Pairs = [];
    const headers: Pairs = [];
    if (options.versionId !== undefined) query.push(["versionId", options.versionId]);
    if (method === "PUT") {
      if (options.type !== undefined) headers.push(["content-type", options.type]);
      if (options.acl !== undefined) query.push(["x-amz-acl", options.acl]);
    }
    if (options.contentDisposition !== undefined && method === "GET") {
      query.push(["response-content-disposition", options.contentDisposition]);
    }
    if (options.responseType !== undefined && method === "GET") {
      query.push(["response-content-type", options.responseType]);
    }
    return this.#core.presign({ method, bucket, key, query, headers }, expiresIn);
  }

  /** One page of the bucket's keys (ListObjectsV2). */
  async list(input: S3ListOptions = {}, options: S3CallOptions = {}): Promise<S3ListResult> {
    const bucket = this.#bucketOf(options.bucket);
    const query: Pairs = [
      ["list-type", "2"],
      // Keys come back URL-encoded, so a key XML cannot carry — or one with
      // leading or trailing spaces, which an XML reader trims — survives.
      ["encoding-type", "url"],
    ];
    if (input.prefix !== undefined) query.push(["prefix", input.prefix]);
    if (input.delimiter !== undefined) query.push(["delimiter", input.delimiter]);
    if (input.maxKeys !== undefined) {
      if (!Number.isInteger(input.maxKeys) || input.maxKeys < 1 || input.maxKeys > 1000) {
        throw invalid("maxKeys is a whole number from 1 to 1000");
      }
      query.push(["max-keys", String(input.maxKeys)]);
    }
    if (input.startAfter !== undefined) query.push(["start-after", input.startAfter]);
    if (input.continuationToken !== undefined)
      query.push(["continuation-token", input.continuationToken]);
    if (input.fetchOwner) query.push(["fetch-owner", "true"]);

    const response = await this.#core.send({
      method: "GET",
      bucket,
      query,
      signal: options.signal,
    });
    const root = parse(await response.text(), `ListObjectsV2 ${bucket}`).ListBucketResult;
    if (root === undefined) {
      throw new S3Error("ListObjectsV2 returned no ListBucketResult", {
        code: S3ErrorCode.Response,
        bucket,
      });
    }
    const encoded = childText(root, "EncodingType") === "url";
    return {
      name: childText(root, "Name"),
      prefix: decodeKey(childText(root, "Prefix"), encoded),
      delimiter: decodeKey(childText(root, "Delimiter"), encoded),
      maxKeys: childNumber(root, "MaxKeys"),
      keyCount: childNumber(root, "KeyCount"),
      isTruncated: childBoolean(root, "IsTruncated") ?? false,
      continuationToken: childText(root, "ContinuationToken"),
      nextContinuationToken: childText(root, "NextContinuationToken"),
      startAfter: decodeKey(childText(root, "StartAfter"), encoded),
      contents: children(root, "Contents").map((item) => {
        const owner = child(item, "Owner");
        return {
          key: decodeKey(childText(item, "Key"), encoded) ?? "",
          size: childNumber(item, "Size") ?? 0,
          lastModified: childDate(item, "LastModified"),
          eTag: childText(item, "ETag"),
          storageClass: childText(item, "StorageClass"),
          checksumAlgorithm: childText(item, "ChecksumAlgorithm"),
          owner:
            owner === undefined
              ? undefined
              : { id: childText(owner, "ID"), displayName: childText(owner, "DisplayName") },
        };
      }),
      commonPrefixes: children(root, "CommonPrefixes").map((item) => ({
        prefix: decodeKey(childText(item, "Prefix"), encoded) ?? "",
      })),
    };
  }

  /** Every key under `input.prefix`, page after page. */
  async *listAll(
    input: Omit<S3ListOptions, "continuationToken"> = {},
    options: S3CallOptions = {},
  ): AsyncGenerator<S3ListObject> {
    let continuationToken: string | undefined;
    do {
      const page = await this.list(
        continuationToken === undefined ? input : { ...input, continuationToken },
        options,
      );
      yield* page.contents;
      continuationToken = page.isTruncated ? page.nextContinuationToken : undefined;
    } while (continuationToken !== undefined);
  }

  /**
   * Copies `from` to `to` inside S3, without the bytes passing through this
   * process. Up to 5 GiB, S3's limit for one copy.
   */
  async copy(from: string, to: string, options: S3CopyOptions = {}): Promise<S3CopyResult> {
    const bucket = this.#bucketOf(options.bucket);
    const sourceBucket = checkBucket(options.sourceBucket ?? bucket);
    checkKey(from);
    checkKey(to);
    const merged: S3CopyOptions = { ...this.#defaults, ...options };
    const https = this.#core.isHttps(bucket);
    let source = `${uriEncode(sourceBucket)}/${uriEncodePath(from)}`;
    if (options.sourceVersionId !== undefined)
      source += `?versionId=${uriEncode(options.sourceVersionId)}`;
    const headers: Pairs = [["x-amz-copy-source", source]];
    if (options.sourceIfMatch !== undefined)
      headers.push(["x-amz-copy-source-if-match", options.sourceIfMatch]);
    const replace =
      options.replaceMetadata || options.metadata !== undefined || options.type !== undefined;
    if (replace) {
      headers.push(
        ["x-amz-metadata-directive", "REPLACE"],
        ...objectHeaders(to, merged, undefined),
      );
    } else {
      // Without REPLACE, S3 keeps the source's type and metadata; only these
      // describe the new copy itself.
      if (merged.acl !== undefined) headers.push(["x-amz-acl", merged.acl]);
      if (merged.storageClass !== undefined)
        headers.push(["x-amz-storage-class", merged.storageClass]);
      if (merged.sse !== undefined) headers.push(["x-amz-server-side-encryption", merged.sse]);
      if (merged.sseKmsKeyId !== undefined) {
        headers.push(["x-amz-server-side-encryption-aws-kms-key-id", merged.sseKmsKeyId]);
      }
      if (merged.requestPayer) headers.push(["x-amz-request-payer", "requester"]);
    }
    headers.push(
      ...conditionalWriteHeaders(options),
      ...sseCustomerHeaders(options.sseCustomerKey, https),
    );
    const response = await this.#core.send({
      method: "PUT",
      bucket,
      key: to,
      headers,
      signal: options.signal,
      retry: options.retry,
      errorIn200: true,
    });
    const root = parse(await response.text(), `CopyObject ${to}`).CopyObjectResult;
    return {
      etag: childText(root, "ETag") ?? "",
      lastModified: childDate(root, "LastModified"),
      versionId: response.headers.get("x-amz-version-id") ?? undefined,
    };
  }

  /**
   * Deletes many keys, a thousand per request (DeleteObjects). Resolves with
   * the keys S3 could not delete; a missing key is not one of them.
   */
  async deleteMany(
    keys: ReadonlyArray<string | { key: string; versionId?: string }>,
    options: S3CallOptions & { requestPayer?: boolean } = {},
  ): Promise<{ errors: S3DeleteError[] }> {
    const bucket = this.#bucketOf(options.bucket);
    const objects = keys.map((entry) => {
      const key = checkKey(typeof entry === "string" ? entry : entry?.key);
      const versionId = typeof entry === "string" ? undefined : entry.versionId;
      return versionId === undefined ? { Key: key } : { Key: key, VersionId: versionId };
    });
    const errors: S3DeleteError[] = [];
    for (let at = 0; at < objects.length; at += 1000) {
      const body = encoder.encode(
        build({
          Delete: { "@xmlns": NAMESPACE, Object: objects.slice(at, at + 1000), Quiet: true },
        }),
      );
      const headers: Pairs = [
        ["content-type", "application/xml"],
        ["content-md5", hash("md5", body, "base64")],
      ];
      if (options.requestPayer) headers.push(["x-amz-request-payer", "requester"]);
      const response = await this.#core.send({
        method: "POST",
        bucket,
        query: [["delete", ""]],
        headers,
        body,
        signal: options.signal,
      });
      const root = parse(await response.text(), `DeleteObjects ${bucket}`).DeleteResult;
      for (const item of children(root, "Error")) {
        errors.push({
          key: childText(item, "Key") ?? "",
          versionId: childText(item, "VersionId") || undefined,
          code: childText(item, "Code") ?? "",
          message: childText(item, "Message") ?? "",
        });
      }
    }
    return { errors };
  }
}
