# @opentf/esrun-s3

An S3 client for [esrun](https://esrun.opentechf.org): AWS S3, Cloudflare R2,
Google Cloud Storage and any S3-compatible store. Written in JavaScript over
`fetch` and signed with SigV4; there is no native code in this package.

The API is Bun's `S3Client` and `S3File`, so code written for `Bun.s3` reads the
same here. [Differences from Bun](#differences-from-bun) lists the few changes.

```sh
npm install @opentf/esrun-s3
```

```js
import { S3Client } from "@opentf/esrun-s3";

const s3 = new S3Client({
  bucket: "uploads",
  region: "eu-west-1",
  accessKeyId: "AKIA…",
  secretAccessKey: "…",
});

await s3.write("reports/q3.json", JSON.stringify(report));
const text = await s3.file("reports/q3.json").text();
const url = await s3.presign("reports/q3.json", { expiresIn: 3600 });
```

Run it with the network grant scoped to the endpoint:

```sh
esrun --allow-net=s3.eu-west-1.amazonaws.com app.js
```

Requires esrun 0.47.0 or later.

## The client

```js
new S3Client(options);
await S3Client.fromEnv(options);
```

| Option | Default | |
| --- | --- | --- |
| `accessKeyId`, `secretAccessKey`, `sessionToken` | — | Strings, or `Secret`s from `runtime:process`'s `env`. |
| `credentials` | — | `{ accessKeyId, secretAccessKey, sessionToken?, expiration? }`, or a function returning them (sync or async). Refreshed five minutes before `expiration`. |
| `bucket` | — | The default bucket. Every call can name another with `{ bucket }`. |
| `region` | `"us-east-1"` | `"auto"` for R2. |
| `endpoint` | AWS's regional endpoint | `https://<account>.r2.cloudflarestorage.com`, `https://storage.googleapis.com`, `http://localhost:9000`. A path in it is kept. |
| `virtualHostedStyle` | `false` | `false`: `endpoint/bucket/key`. `true`: the bucket is in the host; with `endpoint` set, that URL is the bucket's own host and `bucket` is not added to it. |
| `retry` | `3` | Retries after a failed request, 0 to 255. |
| `partSize` | 5 MiB | Multipart part size, 5 MiB to 5 GiB. |
| `queueSize` | `5` | Parts uploaded at once, 1 to 255. |
| [write options](#write-options) | — | Defaults for every write. |

`S3Client.fromEnv()` reads `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY`,
`S3_SESSION_TOKEN`, `S3_REGION`, `S3_ENDPOINT` and `S3_BUCKET`, each falling
back to `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`,
`AWS_REGION`, `AWS_ENDPOINT_URL_S3` / `AWS_ENDPOINT_URL` / `AWS_ENDPOINT` and
`AWS_BUCKET`. Explicit options win. It needs `--allow-env` for those names; a
name not granted reads as unset. The constructor never reads the environment.

| Method | Resolves with |
| --- | --- |
| `file(key, options?)` | an `S3File` (sends nothing) |
| `write(key, data, options?)` | `{ etag, versionId, size }` |
| `writer(key, options?)` | an `S3Writer` |
| `exists(key, options?)` | `boolean` |
| `stat(key, options?)` | [`S3Stats`](#stat) |
| `size(key, options?)` | `number` |
| `delete(key, options?)` | — |
| `presign(key, options?)` | a URL string |
| `list(input?, options?)` | one page: [`S3ListResult`](#list) |
| `listAll(input?, options?)` | an async iterator over every object |
| `copy(from, to, options?)` | `{ etag, lastModified, versionId }` |
| `deleteMany(keys, options?)` | `{ errors: [{ key, versionId, code, message }] }` |

## Files

`s3.file(key, options)` names an object. The options are the
[read options](#read-options) and [write options](#write-options), used by
every method of that file.

```js
const file = s3.file("photos/cat.jpg");

await file.text();           // string
await file.json();           // parsed JSON
await file.bytes();          // Uint8Array
await file.arrayBuffer();
await file.blob();
file.stream();               // ReadableStream<Uint8Array>, sent on first read

await file.exists();
await file.stat();
await file.delete();
await file.write(data, options?);
```

`file.key` is the key (`file.name` too, as in Bun); `file.bucket` the bucket.

The bytes read are the bytes stored. An object written with
`contentEncoding: "gzip"` reads back gzipped, with no decoding, so its size and
ETag match.

### Ranges

`slice(start, end)` follows `Blob.slice`: `start` is inclusive, `end` exclusive,
and a negative `start` counts from the end. Reading a slice sends a `Range`
request.

```js
await file.slice(0, 1024).bytes();   // the first KiB
await file.slice(-100).text();       // the last 100 bytes
await file.slice(4096).stream();     // from offset 4096 on
```

A slice past the end reads as empty. A negative `end` needs the object's size,
so it is refused: call `stat()` first and pass positive offsets.

### Stat

`stat()` resolves with `{ size, lastModified, etag, type, metadata, versionId,
storageClass, contentEncoding, contentDisposition, cacheControl }`. `etag` is
quoted, as S3 returns it; `metadata` holds the `x-amz-meta-*` headers without
the prefix. `exists()` is `false` for a 404 and throws on any other failure.

## Writing

```js
await s3.write("a.txt", "text");
await s3.write("b.bin", new Uint8Array([1, 2, 3]));
await s3.write("c.png", blob);
await s3.write("d.csv", readableStream);
await s3.write("e.html", await fetch(url));     // a Response
await s3.write("f.txt", s3.file("a.txt"));      // another S3File
```

A string, bytes, or a `Blob` up to `partSize` is one PUT. A larger `Blob`, a
`ReadableStream`, a `Response`, a `Request` or an `S3File` is a multipart upload,
holding at most `partSize × (queueSize + 1)` bytes. A stream that ends before
the first part fills is a single PUT.

The `Content-Type` is the `type` option if given. Otherwise it is the `Blob`'s
or `Response`'s own type, then the type a common extension implies (`.json`,
`.html`, `.png`, …), then `text/plain;charset=utf-8` for a string, then
`application/octet-stream`.

### Writers

```js
const writer = s3.file("logs/today.ndjson").writer({ partSize: 8 * 1024 * 1024 });
for await (const line of lines) await writer.write(line);
const { etag } = await writer.end();
```

`write(chunk)` takes a string, an `ArrayBuffer` or a typed array, and waits while
`queueSize` parts are in flight. `end()` sends what is left and completes the
upload. `abort()` abandons it. A part that still fails after its retries aborts
the whole upload, so no parts are left behind on the server.

`file.writable(options)` is the same as a `WritableStream`:

```js
await response.body.pipeTo(s3.file("mirror.bin").writable());
```

### Write options

| Option | |
| --- | --- |
| `type` | `Content-Type`. |
| `acl` | `"private"`, `"public-read"`, `"bucket-owner-full-control"`, … |
| `storageClass` | `"STANDARD"`, `"STANDARD_IA"`, `"INTELLIGENT_TIERING"`, `"GLACIER"`, … |
| `contentDisposition`, `contentEncoding`, `cacheControl` | The headers of the same names. `contentEncoding` describes the bytes; nothing is compressed for you. |
| `metadata` | `{ name: value }`, sent as `x-amz-meta-*`. ASCII only. |
| `checksum` | `"crc32"`, `"crc32c"`, `"sha1"` or `"sha256"`: computed here, verified and stored by S3. Per part for a multipart upload. |
| `sse`, `sseKmsKeyId` | `"AES256"`, `"aws:kms"` or `"aws:kms:dsse"`, and the KMS key. |
| `sseCustomerKey` | SSE-C: a 32-byte `Uint8Array`. Sent only over HTTPS. |
| `ifNoneMatch` | `"*"`: write only if no object has the key. |
| `ifMatch` | Write only if the current object's ETag matches. |
| `requestPayer` | `true` for a requester-pays bucket. |
| `partSize`, `queueSize`, `retry`, `signal` | As on the client, for this write. |

```js
// Create-only: fails with PreconditionFailed if the key exists.
await s3.write("locks/job-42", "", { ifNoneMatch: "*" });

// Compare-and-swap on the ETag.
const { etag } = await s3.stat("counter.json");
await s3.write("counter.json", next, { ifMatch: etag });
```

### Read options

| Option | |
| --- | --- |
| `versionId` | Read a specific version. |
| `ifMatch`, `ifNoneMatch` | Conditional on the ETag: `PreconditionFailed`, or `NotModified` (304). |
| `ifModifiedSince`, `ifUnmodifiedSince` | `Date`s. |
| `sseCustomerKey` | The SSE-C key the object was written with. |
| `requestPayer`, `retry`, `signal` | |

## List

```js
const page = await s3.list({ prefix: "photos/", delimiter: "/", maxKeys: 100 });
page.contents;        // [{ key, size, lastModified, eTag, storageClass, checksumAlgorithm, owner }]
page.commonPrefixes;  // [{ prefix: "photos/2026/" }, …]
page.isTruncated;     // more pages?
page.nextContinuationToken;

for await (const object of s3.listAll({ prefix: "photos/" })) {
  console.log(object.key, object.size);
}
```

`input` takes `prefix`, `delimiter`, `maxKeys` (1 to 1000), `startAfter`,
`continuationToken` and `fetchOwner`. Keys come back exactly as stored,
including ones with spaces at either end.

## Copy and delete

```js
await s3.copy("drafts/a.md", "published/a.md");
await s3.copy("a.md", "b.md", { sourceBucket: "other", metadata: { reviewed: "yes" } });

const { errors } = await s3.deleteMany(["a", "b", { key: "c", versionId: "…" }]);
```

`copy` runs inside S3, so the bytes never pass through this process, up to S3's
5 GiB limit for one copy. Passing `metadata` or `type` replaces the source's
metadata; otherwise it is copied. `deleteMany` sends a thousand keys per
request and resolves with the keys S3 could not delete. Deleting a missing key
is not an error.

## Presigned URLs

```js
const download = await s3.presign("report.pdf", {
  expiresIn: 600,
  contentDisposition: 'attachment; filename="report.pdf"',
});
const upload = await s3.file("inbox/photo.jpg").presign({
  method: "PUT",
  type: "image/jpeg",
  expiresIn: 300,
});
```

| Option | Default | |
| --- | --- | --- |
| `expiresIn` | `86400` | Seconds, 1 to 604800 (7 days). A link signed with temporary credentials stops working when they expire. |
| `method` | `"GET"` | `"GET"`, `"PUT"`, `"DELETE"`, `"HEAD"`, `"POST"`. |
| `type` | — | PUT: the `Content-Type` the uploader must send. |
| `acl` | — | PUT: the ACL the object gets. |
| `contentDisposition`, `responseType` | — | GET: the `Content-Disposition` and `Content-Type` S3 answers with. |
| `versionId` | — | A specific version. |

## Errors

Every failure is an `S3Error` with `code`, `status`, `requestId`, `hostId`,
`bucket`, `key` and, for a wrong-region redirect, `region`.

When S3 answered, `code` is S3's own: `NoSuchKey`, `NoSuchBucket`,
`AccessDenied`, `PreconditionFailed`, `SignatureDoesNotMatch`, `SlowDown`, …. A
`HEAD` has no body, so its code comes from the status: `NotFound`, `Forbidden`,
`PreconditionFailed`, `NotModified`. When S3 did not answer, `code` is one of
`S3ErrorCode`:

| Code | |
| --- | --- |
| `ERR_S3_INVALID_ARGUMENT` | Refused before sending: a bad key, option or size. |
| `ERR_S3_CREDENTIALS` | No credentials, or the provider failed. |
| `ERR_S3_NETWORK` | The connection failed after its retries. |
| `ERR_S3_RESPONSE` | S3 answered with something unreadable. |
| `ERR_S3_ABORTED` | A multipart upload was aborted. |

```js
import { S3Error } from "@opentf/esrun-s3";

try {
  await s3.file("missing").text();
} catch (error) {
  if (error instanceof S3Error && error.code === "NoSuchKey") return null;
  throw error;
}
```

Retried: 429, 5xx, `SlowDown`, `RequestTimeout`, `InternalError`,
`ServiceUnavailable` and connection failures, with exponential backoff.
`RequestTimeTooSkewed` is retried after correcting for the server's clock, and
`ExpiredToken` after asking the credentials provider again. A redirect is never
followed: the error names the bucket's region (`region: "eu-west-1"`).

Keys with a `.` or `..` segment are refused, because a URL cannot carry them as
they are. So are keys over 1024 bytes of UTF-8.

## Providers

| | |
| --- | --- |
| AWS S3 | `region`; the endpoint is derived. |
| Cloudflare R2 | `endpoint: "https://<account-id>.r2.cloudflarestorage.com"`, `region: "auto"`. |
| Google Cloud Storage | `endpoint: "https://storage.googleapis.com"`, with the project's HMAC keys as the access key and secret. |
| Other S3-compatible stores | Their endpoint; path-style is the default. The test suite runs against RustFS. |

What each service implements varies: R2, for one, does not implement `x-amz-acl`
on a copy. The service's own S3 compatibility page is the reference.

## Differences from Bun

- **The environment is read only by `S3Client.fromEnv()`.** `new S3Client()`
  with no credentials fails rather than picking up whatever the process
  inherited.
- **`presign()` returns a promise**, because credentials may come from a
  provider.
- **`write()` resolves with `{ etag, versionId, size }`**, not a byte count.
- **`lastModified` in `list()` is a `Date`**, as it is in `stat()`.
- **`list()` has no `encodingType` option.** Keys are always requested
  URL-encoded and decoded for you.
