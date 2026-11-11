/**
 * What can go wrong talking to S3.
 *
 * Two vocabularies meet here. When the service answered, `code` is S3's own —
 * `NoSuchKey`, `AccessDenied`, `PreconditionFailed`, `SlowDown` — the names its
 * documentation and every other SDK use, so an application can branch on them
 * and search for them. A response without a body (every `HEAD`) has no code of
 * its own, so one is named from the status: `NotFound`, `Forbidden`,
 * `PreconditionFailed`, `NotModified`. When the request never got an answer, or
 * the client refused it before sending, `code` is one of this package's
 * `ERR_S3_*` codes.
 */

/** This package's own codes, for failures S3 did not name. */
export const S3ErrorCode = {
  /** An argument was refused before anything was sent: a bad key, option or size. */
  InvalidArgument: "ERR_S3_INVALID_ARGUMENT",
  /** No credentials, or a credentials provider failed or returned something unusable. */
  Credentials: "ERR_S3_CREDENTIALS",
  /** The request could not be sent or its response could not be read. */
  Network: "ERR_S3_NETWORK",
  /** S3 answered with something this client could not understand. */
  Response: "ERR_S3_RESPONSE",
  /** A multipart upload was aborted, by the caller or after a failed part. */
  Aborted: "ERR_S3_ABORTED",
} as const;

export interface S3ErrorOptions {
  code: string;
  /** The HTTP status, when S3 answered. */
  status?: number | undefined;
  /** `x-amz-request-id`: what AWS support asks for. */
  requestId?: string | undefined;
  /** `x-amz-id-2`, the extended request id. */
  hostId?: string | undefined;
  bucket?: string | undefined;
  key?: string | undefined;
  /** For a wrong-region answer: the region the bucket is in (`x-amz-bucket-region`). */
  region?: string | undefined;
  cause?: unknown;
}

export class S3Error extends Error {
  override name = "S3Error";
  readonly code: string;
  readonly status: number | undefined;
  readonly requestId: string | undefined;
  readonly hostId: string | undefined;
  readonly bucket: string | undefined;
  readonly key: string | undefined;
  readonly region: string | undefined;

  constructor(message: string, options: S3ErrorOptions) {
    super(message, options.cause === undefined ? undefined : { cause: options.cause });
    this.code = options.code;
    this.status = options.status;
    this.requestId = options.requestId;
    this.hostId = options.hostId;
    this.bucket = options.bucket;
    this.key = options.key;
    this.region = options.region;
  }
}

export function invalid(message: string): S3Error {
  return new S3Error(message, { code: S3ErrorCode.InvalidArgument });
}
