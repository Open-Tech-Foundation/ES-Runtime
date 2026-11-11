/**
 * Options shared by reads and writes, and the headers they become.
 */
import { hash } from "runtime:hashing";
import type { Pairs } from "./core.js";
import { invalid } from "./errors.js";

export type Acl =
  | "private"
  | "public-read"
  | "public-read-write"
  | "aws-exec-read"
  | "authenticated-read"
  | "bucket-owner-read"
  | "bucket-owner-full-control"
  | "log-delivery-write";

export type StorageClass =
  | "STANDARD"
  | "REDUCED_REDUNDANCY"
  | "STANDARD_IA"
  | "ONEZONE_IA"
  | "INTELLIGENT_TIERING"
  | "GLACIER"
  | "DEEP_ARCHIVE"
  | "OUTPOSTS"
  | "GLACIER_IR"
  | "SNOW"
  | "EXPRESS_ONEZONE"
  | (string & {});

/** A checksum S3 stores with the object and verifies on upload. */
export type ChecksumAlgorithm = "crc32" | "crc32c" | "sha1" | "sha256";

export interface ReadOptions {
  /** Read this version of the object rather than the current one. */
  versionId?: string;
  /** Read only if the object's ETag matches; otherwise `PreconditionFailed`. */
  ifMatch?: string;
  /** Read only if the ETag differs; otherwise `NotModified`. */
  ifNoneMatch?: string;
  ifModifiedSince?: Date;
  ifUnmodifiedSince?: Date;
  /**
   * The 256-bit key an object was written with under SSE-C. Sent only over
   * HTTPS: the client refuses to put a key on a plain-text connection.
   */
  sseCustomerKey?: Uint8Array;
  /** Pay for the request as the requester (`x-amz-request-payer`). */
  requestPayer?: boolean;
  signal?: AbortSignal;
}

export interface WriteOptions {
  /** `Content-Type`. Defaults to one inferred from the key's extension, else `application/octet-stream`. */
  type?: string;
  acl?: Acl;
  storageClass?: StorageClass;
  contentDisposition?: string;
  /** The coding the bytes are already in (`gzip` for a pre-compressed file). Nothing is compressed for you. */
  contentEncoding?: string;
  cacheControl?: string;
  /** User metadata, sent as `x-amz-meta-*`. ASCII names and values. */
  metadata?: Record<string, string>;
  /** A checksum for S3 to verify and store. */
  checksum?: ChecksumAlgorithm;
  /** Server-side encryption with S3-managed or KMS keys. */
  sse?: "AES256" | "aws:kms" | "aws:kms:dsse";
  /** The KMS key for `sse: "aws:kms"`. */
  sseKmsKeyId?: string;
  /** SSE-C: encrypt with this 256-bit key. Sent only over HTTPS. */
  sseCustomerKey?: Uint8Array;
  /** Write only if the current object's ETag matches. */
  ifMatch?: string;
  /** `"*"`: write only if no object has the key. */
  ifNoneMatch?: string;
  requestPayer?: boolean;
  /** Multipart part size in bytes: 5 MiB (the default) to 5 GiB. */
  partSize?: number;
  /** Parts uploaded at once: 1 to 255, default 5. */
  queueSize?: number;
  /** Retries after a failed request. Default 3. */
  retry?: number;
  signal?: AbortSignal;
}

const MIB = 1024 * 1024;
export const MIN_PART: number = 5 * MIB;
export const MAX_PART: number = 5 * 1024 * MIB;
export const MAX_PARTS: number = 10_000;

export function partSizeOf(value: number | undefined): number {
  const size = value ?? MIN_PART;
  if (!Number.isInteger(size) || size < MIN_PART || size > MAX_PART) {
    throw invalid(`partSize is a whole number of bytes from 5 MiB to 5 GiB (got ${value})`);
  }
  return size;
}

export function queueSizeOf(value: number | undefined): number {
  const size = value ?? 5;
  if (!Number.isInteger(size) || size < 1 || size > 255) {
    throw invalid(`queueSize is a whole number from 1 to 255 (got ${value})`);
  }
  return size;
}

export function retryOf(value: number | undefined, fallback: number): number {
  const retry = value ?? fallback;
  if (!Number.isInteger(retry) || retry < 0 || retry > 255) {
    throw invalid(`retry is a whole number from 0 to 255 (got ${value})`);
  }
  return retry;
}

const TYPES: Record<string, string> = {
  html: "text/html;charset=utf-8",
  htm: "text/html;charset=utf-8",
  css: "text/css;charset=utf-8",
  js: "text/javascript;charset=utf-8",
  mjs: "text/javascript;charset=utf-8",
  json: "application/json",
  map: "application/json",
  txt: "text/plain;charset=utf-8",
  md: "text/markdown;charset=utf-8",
  csv: "text/csv;charset=utf-8",
  xml: "application/xml",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  avif: "image/avif",
  ico: "image/x-icon",
  pdf: "application/pdf",
  zip: "application/zip",
  gz: "application/gzip",
  tar: "application/x-tar",
  wasm: "application/wasm",
  woff: "font/woff",
  woff2: "font/woff2",
  mp3: "audio/mpeg",
  mp4: "video/mp4",
  webm: "video/webm",
};

/** The `Content-Type` a key's extension implies, if it is a common one. */
export function typeOf(key: string): string | undefined {
  const name = key.slice(key.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? TYPES[name.slice(dot + 1).toLowerCase()] : undefined;
}

const TOKEN = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
const ASCII = /^[\x20-\x7e]*$/;

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/** The three SSE-C headers, refused unless the connection is HTTPS. */
export function sseCustomerHeaders(key: Uint8Array | undefined, https: boolean): Pairs {
  if (key === undefined) return [];
  if (!(key instanceof Uint8Array) || key.length !== 32) {
    throw invalid("sseCustomerKey is a 32-byte Uint8Array (AES-256)");
  }
  if (!https) throw invalid("sseCustomerKey is sent only over HTTPS; the endpoint is plain HTTP");
  return [
    ["x-amz-server-side-encryption-customer-algorithm", "AES256"],
    ["x-amz-server-side-encryption-customer-key", base64(key)],
    ["x-amz-server-side-encryption-customer-key-md5", hash("md5", key, "base64")],
  ];
}

export function readHeaders(options: ReadOptions, https: boolean): Pairs {
  const out: Pairs = [];
  if (options.ifMatch !== undefined) out.push(["if-match", options.ifMatch]);
  if (options.ifNoneMatch !== undefined) out.push(["if-none-match", options.ifNoneMatch]);
  if (options.ifModifiedSince !== undefined)
    out.push(["if-modified-since", options.ifModifiedSince.toUTCString()]);
  if (options.ifUnmodifiedSince !== undefined) {
    out.push(["if-unmodified-since", options.ifUnmodifiedSince.toUTCString()]);
  }
  if (options.requestPayer) out.push(["x-amz-request-payer", "requester"]);
  out.push(...sseCustomerHeaders(options.sseCustomerKey, https));
  return out;
}

/** Headers describing the object: what CreateMultipartUpload and PutObject both take. */
export function objectHeaders(
  key: string,
  options: WriteOptions,
  fallbackType: string | undefined,
): Pairs {
  const out: Pairs = [];
  out.push([
    "content-type",
    options.type ?? fallbackType ?? typeOf(key) ?? "application/octet-stream",
  ]);
  if (options.acl !== undefined) out.push(["x-amz-acl", options.acl]);
  if (options.storageClass !== undefined) out.push(["x-amz-storage-class", options.storageClass]);
  if (options.contentDisposition !== undefined)
    out.push(["content-disposition", options.contentDisposition]);
  if (options.contentEncoding !== undefined)
    out.push(["content-encoding", options.contentEncoding]);
  if (options.cacheControl !== undefined) out.push(["cache-control", options.cacheControl]);
  if (options.sse !== undefined) out.push(["x-amz-server-side-encryption", options.sse]);
  if (options.sseKmsKeyId !== undefined) {
    out.push(["x-amz-server-side-encryption-aws-kms-key-id", options.sseKmsKeyId]);
  }
  if (options.requestPayer) out.push(["x-amz-request-payer", "requester"]);
  for (const [name, value] of Object.entries(options.metadata ?? {})) {
    if (!TOKEN.test(name)) throw invalid(`metadata name "${name}" is not an HTTP token`);
    if (typeof value !== "string" || !ASCII.test(value)) {
      throw invalid(`metadata "${name}" must be printable ASCII`);
    }
    out.push([`x-amz-meta-${name.toLowerCase()}`, value]);
  }
  return out;
}

export function conditionalWriteHeaders(options: WriteOptions): Pairs {
  const out: Pairs = [];
  if (options.ifMatch !== undefined) out.push(["if-match", options.ifMatch]);
  if (options.ifNoneMatch !== undefined) out.push(["if-none-match", options.ifNoneMatch]);
  return out;
}

const CHECKSUMS: Record<ChecksumAlgorithm, string> = {
  crc32: "CRC32",
  crc32c: "CRC32C",
  sha1: "SHA1",
  sha256: "SHA256",
};

export function checksumName(algorithm: ChecksumAlgorithm | undefined): string | undefined {
  if (algorithm === undefined) return undefined;
  const name = CHECKSUMS[algorithm];
  if (name === undefined)
    throw invalid(`checksum is one of crc32, crc32c, sha1, sha256 (got ${algorithm})`);
  return name;
}

/** `x-amz-checksum-<alg>`: the base64 of the digest (big-endian for the CRCs). */
export function checksumHeader(algorithm: ChecksumAlgorithm | undefined, body: Uint8Array): Pairs {
  if (checksumName(algorithm) === undefined) return [];
  return [[`x-amz-checksum-${algorithm}`, hash(algorithm!, body, "base64")]];
}

export function checksumValue(algorithm: ChecksumAlgorithm, body: Uint8Array): string {
  return hash(algorithm, body, "base64");
}
