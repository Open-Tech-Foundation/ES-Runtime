/**
 * One signed request to S3: where it goes, how it is signed, what happens when
 * it fails, and the error it becomes.
 *
 * Every body is held in memory before it is sent (a multipart upload holds one
 * part at a time), so every request is signed with the SHA-256 of its body and
 * every request can be retried. Nothing is sent as `UNSIGNED-PAYLOAD` except a
 * presigned URL, whose body is not known when it is signed.
 */
import { hash } from "runtime:hashing";
import type { CredentialCache } from "./credentials.js";
import { invalid, S3Error, S3ErrorCode } from "./errors.js";
import {
  EMPTY_SHA256,
  encodeQuery,
  SigningKeys,
  signHeaders,
  signQuery,
  uriEncode,
  uriEncodePath,
} from "./sigv4.js";
import { childText, parse } from "./xml.js";

export type Method = "GET" | "PUT" | "POST" | "DELETE" | "HEAD";
export type Pairs = Array<[string, string]>;

export interface RequestSpec {
  method: Method;
  bucket: string;
  /** Absent for a bucket-level request (list, multi-object delete). */
  key?: string | undefined;
  query?: Pairs;
  headers?: Pairs;
  body?: Uint8Array | undefined;
  signal?: AbortSignal | undefined;
  /** S3 can answer 200 with an `<Error>` body (CompleteMultipartUpload, CopyObject). */
  errorIn200?: boolean;
  /** Retries after the first attempt. */
  retry?: number | undefined;
  /** Statuses that are not errors beyond 2xx (416 for a range past the end). */
  accept?: number[];
}

export interface CoreConfig {
  /** The endpoint as given, or `undefined` for AWS's own. */
  endpoint: URL | undefined;
  region: string;
  virtualHostedStyle: boolean;
  retry: number;
  credentials: CredentialCache;
}

/** Backoff, in milliseconds. Exported for the tests; not part of the package's API. */
export const backoff = { base: 100, throttledBase: 1000, cap: 20_000 };

const RETRY_STATUS = new Set([429, 500, 502, 503, 504]);
const RETRY_CODES = new Set([
  "SlowDown",
  "RequestTimeout",
  "InternalError",
  "ServiceUnavailable",
  "RequestTimeTooSkewed",
  "ExpiredToken",
  S3ErrorCode.Network,
]);
const THROTTLED = new Set(["SlowDown", "ServiceUnavailable"]);

const STATUS_CODES: Record<number, string> = {
  304: "NotModified",
  400: "BadRequest",
  403: "Forbidden",
  404: "NotFound",
  405: "MethodNotAllowed",
  409: "Conflict",
  412: "PreconditionFailed",
  416: "InvalidRange",
  429: "SlowDown",
  500: "InternalError",
  503: "ServiceUnavailable",
};

const BUCKET = /^[A-Za-z0-9._-]{1,255}$/;

export function checkBucket(bucket: unknown): string {
  if (typeof bucket !== "string" || bucket === "") {
    throw invalid("no bucket: pass bucket to the client, or to this call");
  }
  if (!BUCKET.test(bucket) || bucket === "." || bucket === "..") {
    throw invalid(`"${bucket}" is not a bucket name`);
  }
  return bucket;
}

const encoder = new TextEncoder();

/**
 * A key, refused when it cannot be sent as itself.
 *
 * A `.` or `..` segment is the case that matters: the URL parser resolves it,
 * percent-encoded or not, so `a/../b` would reach `b` — and in path-style
 * addressing `../other-bucket/x` would reach another bucket.
 */
export function checkKey(key: unknown): string {
  if (typeof key !== "string" || key === "") throw invalid("a key is a non-empty string");
  if (encoder.encode(key).length > 1024) throw invalid("a key is at most 1024 bytes of UTF-8");
  for (const segment of key.split("/")) {
    if (segment === "." || segment === "..") {
      throw invalid(`"${key}" has a "${segment}" segment, which a URL cannot carry`);
    }
  }
  return key;
}

function sleep(ms: number, signal: AbortSignal | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal!.reason);
    };
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

function describe(spec: RequestSpec): string {
  return spec.key === undefined
    ? `${spec.method} ${spec.bucket}`
    : `${spec.method} ${spec.bucket}/${spec.key}`;
}

export class Core {
  readonly config: CoreConfig;
  readonly #keys = new SigningKeys();
  /** Server time minus local time, learned from a `RequestTimeTooSkewed` answer. */
  #skew = 0;

  constructor(config: CoreConfig) {
    this.config = config;
  }

  /** Where a request goes, and the path exactly as it is sent and signed. */
  target(
    bucket: string,
    key: string | undefined,
    query: Pairs,
  ): { url: URL; canonicalUri: string } {
    const { endpoint, region, virtualHostedStyle } = this.config;
    const encodedKey = key === undefined ? "" : uriEncodePath(key);
    let origin: string;
    let path: string;
    if (virtualHostedStyle) {
      // The endpoint, when given, is the bucket's own host (Bun's rule); the
      // bucket name is not repeated in it.
      origin = endpoint ? endpoint.origin : `https://${bucket}.s3.${region}.amazonaws.com`;
      const base = endpoint ? endpoint.pathname.replace(/\/+$/, "") : "";
      path = `${base}/${encodedKey}`;
    } else {
      origin = endpoint ? endpoint.origin : `https://s3.${region}.amazonaws.com`;
      const base = endpoint ? endpoint.pathname.replace(/\/+$/, "") : "";
      path =
        key === undefined
          ? `${base}/${uriEncode(bucket)}`
          : `${base}/${uriEncode(bucket)}/${encodedKey}`;
    }
    const search = query.length > 0 ? `?${encodeQuery(query)}` : "";
    const url = new URL(`${origin}${path}${search}`);
    if (url.pathname !== path) {
      // Never sign one path and send another.
      throw invalid(`the request path ${path} would be sent as ${url.pathname}`);
    }
    return { url, canonicalUri: path };
  }

  isHttps(bucket: string): boolean {
    return this.target(bucket, undefined, []).url.protocol === "https:";
  }

  #now(): Date {
    return new Date(Date.now() + this.#skew);
  }

  /** Sends `spec`, retrying what can be retried, and resolves with a successful response. */
  async send(spec: RequestSpec): Promise<Response> {
    const query = spec.query ?? [];
    const extra = spec.headers ?? [];
    const payloadHash = spec.body ? hash("sha256", spec.body, "hex") : EMPTY_SHA256;
    const attempts = 1 + Math.max(0, spec.retry ?? this.config.retry);
    let last: S3Error | undefined;

    for (let attempt = 0; attempt < attempts; attempt++) {
      if (attempt > 0 && last) {
        const base =
          THROTTLED.has(last.code) || last.status === 429 ? backoff.throttledBase : backoff.base;
        await sleep(Math.random() * Math.min(backoff.cap, base * 2 ** attempt), spec.signal);
      }
      const credentials = await this.config.credentials.get();
      const { url, canonicalUri } = this.target(spec.bucket, spec.key, query);
      const signed = signHeaders(
        { method: spec.method, canonicalUri, query, headers: [...extra, ["host", url.host]] },
        credentials,
        { region: this.config.region, service: "s3", date: this.#now() },
        { payloadHash, contentSha256Header: true, keys: this.#keys },
      );
      const headers = new Headers();
      for (const [name, value] of extra) headers.append(name, value);
      for (const [name, value] of signed.headers) headers.set(name, value);

      let response: Response;
      try {
        response = await fetch(url, {
          method: spec.method,
          headers,
          body: spec.body ?? null,
          signal: spec.signal ?? null,
          // A signed request is signed for one host: a redirect is reported,
          // never followed.
          redirect: "manual",
          // An object's Content-Encoding is part of the object (D210).
          decompress: false,
        } as RequestInit);
      } catch (cause) {
        if (spec.signal?.aborted) throw spec.signal.reason;
        last = new S3Error(`${describe(spec)}: ${(cause as Error)?.message ?? cause}`, {
          code: S3ErrorCode.Network,
          bucket: spec.bucket,
          key: spec.key,
          cause,
        });
        continue;
      }

      let error: S3Error | undefined;
      if (response.ok || spec.accept?.includes(response.status)) {
        if (!spec.errorIn200) return response;
        const body = await this.#read(response, spec);
        if (!/^\s*(<\?xml[^>]*\?>\s*)?<Error[\s>]/.test(body)) {
          return new Response(body, { status: response.status, headers: response.headers });
        }
        error = this.#error(response, spec, body);
      } else {
        error = this.#error(
          response,
          spec,
          spec.method === "HEAD" ? "" : await this.#read(response, spec),
        );
      }

      const retryable =
        RETRY_CODES.has(error.code) ||
        (error.status !== undefined && RETRY_STATUS.has(error.status));
      if (!retryable) throw error;
      if (error.code === "RequestTimeTooSkewed") {
        const server = Date.parse(response.headers.get("date") ?? "");
        if (!Number.isNaN(server)) this.#skew = server - Date.now();
      }
      if (error.code === "ExpiredToken") this.config.credentials.invalidate();
      last = error;
    }
    throw last!;
  }

  async #read(response: Response, spec: RequestSpec): Promise<string> {
    try {
      return await response.text();
    } catch {
      if (spec.signal?.aborted) throw spec.signal.reason;
      return "";
    }
  }

  #error(response: Response, spec: RequestSpec, body: string): S3Error {
    let code: string | undefined;
    let message: string | undefined;
    if (body.trimStart().startsWith("<")) {
      try {
        const root = parse(body, describe(spec));
        const error = root.Error;
        code = childText(error, "Code") || undefined;
        message = childText(error, "Message") || undefined;
      } catch {
        // Not XML after all: fall back to the status.
      }
    }
    code ??= STATUS_CODES[response.status] ?? `Http${response.status}`;
    const region = response.headers.get("x-amz-bucket-region") ?? undefined;
    let text = `${code}: ${message ?? `${response.status} ${response.statusText}`.trim()} (${describe(spec)})`;
    if (response.status >= 300 && response.status < 400) {
      text += region
        ? ` — the bucket is in ${region}; set region: "${region}"`
        : " — the bucket is elsewhere; check region and endpoint";
    }
    return new S3Error(text, {
      code,
      status: response.status,
      requestId: response.headers.get("x-amz-request-id") ?? undefined,
      hostId: response.headers.get("x-amz-id-2") ?? undefined,
      bucket: spec.bucket,
      key: spec.key,
      region,
    });
  }

  /** A presigned URL for `spec`, valid for `expiresIn` seconds. */
  async presign(
    spec: { method: Method; bucket: string; key: string; query: Pairs; headers: Pairs },
    expiresIn: number,
  ): Promise<string> {
    const credentials = await this.config.credentials.get();
    const { url, canonicalUri } = this.target(spec.bucket, spec.key, []);
    const signed = signQuery(
      {
        method: spec.method,
        canonicalUri,
        query: spec.query,
        headers: [...spec.headers, ["host", url.host]],
      },
      credentials,
      { region: this.config.region, service: "s3", date: this.#now() },
      { expiresIn, keys: this.#keys },
    );
    return `${url.origin}${canonicalUri}?${encodeQuery(signed.query)}`;
  }
}
