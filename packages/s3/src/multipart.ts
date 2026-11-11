/**
 * The three requests every multipart upload shares — create, complete, abort —
 * whether its parts are bytes sent from here (`S3Writer`) or ranges copied
 * inside S3 (`copy()` past the single-copy limit).
 */
import type { Core, Pairs } from "./core.js";
import { S3Error, S3ErrorCode } from "./errors.js";
import { build, childText, NAMESPACE, parse } from "./xml.js";

export interface UploadTarget {
  core: Core;
  bucket: string;
  key: string;
  retry: number;
  signal?: AbortSignal | undefined;
}

const encoder = new TextEncoder();

/** CreateMultipartUpload: the upload id, with the object described by `headers`. */
export async function createUpload(target: UploadTarget, headers: Pairs): Promise<string> {
  const response = await target.core.send({
    method: "POST",
    bucket: target.bucket,
    key: target.key,
    query: [["uploads", ""]],
    headers,
    signal: target.signal,
    retry: target.retry,
  });
  const root = parse(await response.text(), `CreateMultipartUpload ${target.key}`);
  const id = childText(root.InitiateMultipartUploadResult, "UploadId");
  if (!id) {
    throw new S3Error("CreateMultipartUpload returned no UploadId", {
      code: S3ErrorCode.Response,
      key: target.key,
    });
  }
  return id;
}

/** CompleteMultipartUpload. `parts` are `{ PartNumber, ETag, … }` in order. */
export async function completeUpload(
  target: UploadTarget,
  uploadId: string,
  parts: Array<Record<string, unknown>>,
  headers: Pairs,
): Promise<{ etag: string; versionId: string | undefined }> {
  const body = encoder.encode(
    build({ CompleteMultipartUpload: { "@xmlns": NAMESPACE, Part: parts } }),
  );
  const response = await target.core.send({
    method: "POST",
    bucket: target.bucket,
    key: target.key,
    query: [["uploadId", uploadId]],
    headers: [["content-type", "application/xml"], ...headers],
    body,
    signal: target.signal,
    retry: target.retry,
    errorIn200: true,
  });
  const root = parse(await response.text(), `CompleteMultipartUpload ${target.key}`);
  return {
    etag: childText(root.CompleteMultipartUploadResult, "ETag") ?? "",
    versionId: response.headers.get("x-amz-version-id") ?? undefined,
  };
}

/**
 * AbortMultipartUpload, best effort: the failure that led here is what the
 * caller needs to see. An upload left behind anyway is what a bucket lifecycle
 * rule (AbortIncompleteMultipartUpload) is for.
 */
export async function abortUpload(target: UploadTarget, uploadId: string): Promise<void> {
  try {
    const response = await target.core.send({
      method: "DELETE",
      bucket: target.bucket,
      key: target.key,
      query: [["uploadId", uploadId]],
      retry: target.retry,
      accept: [404],
    });
    await response.body?.cancel();
  } catch {
    // best effort, as above
  }
}
