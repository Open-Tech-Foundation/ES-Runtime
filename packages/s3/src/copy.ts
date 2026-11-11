/**
 * A copy larger than one CopyObject takes, done inside S3 as a multipart upload
 * whose parts are ranges of the source (UploadPartCopy). The bytes never pass
 * through this process.
 *
 * `copy()` tries one CopyObject first and comes here only when that is refused
 * and the source is over {@link copyLimits}.single: AWS caps one copy at 5 GB,
 * Google Cloud Storage takes up to 5 TiB in one and does not implement
 * UploadPartCopy, so trying the single copy first is what works on both.
 *
 * The parts are pinned to one source. When the source has a version id, every
 * part names it. Otherwise the source's ETag is read before the first part and
 * again before completing, and a change aborts the copy: Cloudflare R2 does not
 * implement `x-amz-copy-source-if-match` on part copies, so the pin cannot rest
 * on that header.
 */
import type { Core, Pairs } from "./core.js";
import { invalid, S3Error, S3ErrorCode } from "./errors.js";
import { abortUpload, completeUpload, createUpload } from "./multipart.js";
import {
  conditionalWriteHeaders,
  MAX_PART,
  MAX_PARTS,
  MIN_PART,
  objectHeaders,
  queueSizeOf,
  retryOf,
  sseCustomerHeaders,
  type WriteOptions,
} from "./options.js";
import { uriEncode, uriEncodePath } from "./sigv4.js";
import { childText, parse } from "./xml.js";

/** Sizes, in bytes. Exported for the tests; not part of the package's API. */
export const copyLimits: { single: number; part: number } = {
  /** The largest object one CopyObject copies on AWS: 5 GiB. */
  single: 5 * 1024 * 1024 * 1024,
  /** Part size when none is given: fewer, larger requests, since no bytes travel here. */
  part: 256 * 1024 * 1024,
};

export interface SourceInfo {
  size: number;
  etag: string;
  versionId: string | undefined;
  headers: Headers;
}

/** HEAD of the source, as the copy sees it. */
export async function headSource(
  core: Core,
  bucket: string,
  key: string,
  versionId: string | undefined,
  options: { retry: number; signal?: AbortSignal | undefined },
): Promise<SourceInfo> {
  const response = await core.send({
    method: "HEAD",
    bucket,
    key,
    query: versionId === undefined ? [] : [["versionId", versionId]],
    signal: options.signal,
    retry: options.retry,
  });
  return {
    size: Number(response.headers.get("content-length") ?? Number.NaN),
    etag: response.headers.get("etag") ?? "",
    versionId: response.headers.get("x-amz-version-id") ?? undefined,
    headers: response.headers,
  };
}

export interface MultipartCopyOptions extends WriteOptions {
  sourceVersionId?: string;
  sourceIfMatch?: string;
  replaceMetadata?: boolean;
}

/** The source's own description, carried to the copy unless it is replaced. */
function sourceDescription(headers: Headers): WriteOptions {
  const metadata: Record<string, string> = {};
  for (const [name, value] of headers) {
    if (name.startsWith("x-amz-meta-")) metadata[name.slice("x-amz-meta-".length)] = value;
  }
  const out: WriteOptions = { metadata };
  const type = headers.get("content-type");
  if (type !== null) out.type = type;
  const cacheControl = headers.get("cache-control");
  if (cacheControl !== null) out.cacheControl = cacheControl;
  const contentEncoding = headers.get("content-encoding");
  if (contentEncoding !== null) out.contentEncoding = contentEncoding;
  const contentDisposition = headers.get("content-disposition");
  if (contentDisposition !== null) out.contentDisposition = contentDisposition;
  return out;
}

export async function multipartCopy(
  core: Core,
  bucket: string,
  to: string,
  sourceBucket: string,
  from: string,
  source: SourceInfo,
  options: MultipartCopyOptions,
  replace: boolean,
): Promise<{ etag: string; versionId: string | undefined }> {
  const retry = retryOf(options.retry, core.config.retry);
  const queueSize = queueSizeOf(options.queueSize);
  const target = { core, bucket, key: to, retry, signal: options.signal };
  const what = `copy ${sourceBucket}/${from} to ${bucket}/${to}`;

  if (!Number.isFinite(source.size)) {
    throw new S3Error(`${what}: the source's size is unknown`, { code: S3ErrorCode.Response });
  }
  if (options.sourceIfMatch !== undefined && options.sourceIfMatch !== source.etag) {
    throw new S3Error(`${what}: the source's ETag is not ${options.sourceIfMatch}`, {
      code: "PreconditionFailed",
      status: 412,
      bucket: sourceBucket,
      key: from,
    });
  }

  // As many parts as S3 allows at most, each at least the size asked for.
  let partSize = options.partSize ?? copyLimits.part;
  if (!Number.isInteger(partSize) || partSize < MIN_PART || partSize > MAX_PART) {
    throw invalid(
      `partSize is a whole number of bytes from 5 MiB to 5 GiB (got ${options.partSize})`,
    );
  }
  partSize = Math.max(partSize, Math.ceil(source.size / MAX_PARTS));
  if (partSize > MAX_PART) throw invalid(`${what}: the source is larger than 10000 parts of 5 GiB`);

  // Pin every part to one version when there is one; otherwise to the ETag,
  // checked again before completing.
  const versionId = options.sourceVersionId ?? source.versionId;
  let copySource = `${uriEncode(sourceBucket)}/${uriEncodePath(from)}`;
  if (versionId !== undefined) copySource += `?versionId=${uriEncode(versionId)}`;

  const https = core.isHttps(bucket);
  const description: WriteOptions = replace
    ? options
    : { ...options, ...sourceDescription(source.headers) };
  const uploadId = await createUpload(target, [
    ...objectHeaders(to, description, undefined),
    ...sseCustomerHeaders(options.sseCustomerKey, https),
  ]);

  try {
    const count = Math.max(1, Math.ceil(source.size / partSize));
    const parts: Array<{ PartNumber: number; ETag: string }> = [];
    let next = 0;
    let failure: unknown;
    const worker = async () => {
      while (failure === undefined && next < count) {
        const index = next++;
        const first = index * partSize;
        const last = Math.min(source.size, first + partSize) - 1;
        const headers: Pairs = [
          ["x-amz-copy-source", copySource],
          ...sseCustomerHeaders(options.sseCustomerKey, https),
        ];
        // A zero-byte source is one empty part, which takes no range.
        if (source.size > 0) headers.push(["x-amz-copy-source-range", `bytes=${first}-${last}`]);
        try {
          const response = await core.send({
            method: "PUT",
            bucket,
            key: to,
            query: [
              ["partNumber", String(index + 1)],
              ["uploadId", uploadId],
            ],
            headers,
            signal: options.signal,
            retry,
            errorIn200: true,
          });
          const etag = childText(
            parse(await response.text(), `UploadPartCopy ${index + 1}`).CopyPartResult,
            "ETag",
          );
          if (!etag) {
            throw new S3Error(`${what}: part ${index + 1} returned no ETag`, {
              code: S3ErrorCode.Response,
            });
          }
          parts[index] = { PartNumber: index + 1, ETag: etag };
        } catch (error) {
          failure ??= error;
        }
      }
    };
    await Promise.all(Array.from({ length: Math.min(queueSize, count) }, worker));
    if (failure !== undefined) throw failure;

    if (versionId === undefined) {
      const now = await headSource(core, sourceBucket, from, undefined, {
        retry,
        signal: options.signal,
      });
      if (now.etag !== source.etag) {
        throw new S3Error(`${what}: the source changed while it was being copied`, {
          code: "PreconditionFailed",
          status: 412,
          bucket: sourceBucket,
          key: from,
        });
      }
    }
    return await completeUpload(target, uploadId, parts, [
      ...conditionalWriteHeaders(options),
      ...sseCustomerHeaders(options.sseCustomerKey, https),
    ]);
  } catch (error) {
    await abortUpload(target, uploadId);
    throw error;
  }
}
