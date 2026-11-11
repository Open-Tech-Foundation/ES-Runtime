// An S3 small enough to own, on runtime:http, for the unit tests.
//
// It checks every request's signature from what arrived — method, raw path,
// query, the signed headers and the body's hash — so a request that was signed
// one way and sent another fails here, as it would against S3. Faults are
// scripted: `fake.fail(match, response)` answers the next matching request
// with `response` instead of handling it.

import { hash } from "runtime:hashing";
import { serve } from "runtime:http";
import { signHeaders } from "../../dist/sigv4.js";

export const ACCESS_KEY = "AKIDFAKE";
export const SECRET_KEY = "fake/secret+key";
const xml = (body, status = 200, headers = {}) =>
  new Response(`<?xml version="1.0" encoding="UTF-8"?>\n${body}`, {
    status,
    headers: { "content-type": "application/xml", ...headers },
  });

const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

export const errorXml = (code, message, status) =>
  xml(
    `<Error><Code>${code}</Code><Message>${esc(message)}</Message><RequestId>req-1</RequestId></Error>`,
    status,
    {
      "x-amz-request-id": "req-1",
    },
  );

/** `encoding-type=url` as S3 writes it: `+` for a space. */
const urlEncodeKey = (key) => encodeURIComponent(key).replace(/%20/g, "+");

function etagOf(bytes) {
  return `"${hash("md5", bytes, "hex")}"`;
}

export async function startFake() {
  /** bucket -> key -> { bytes, headers } */
  const buckets = new Map([["bucket", new Map()]]);
  const uploads = new Map(); // id -> { bucket, key, headers, parts: Map }
  const faults = [];
  const log = [];
  let nextUpload = 1;
  let skewMs = 0;

  function verify(request, path, body) {
    const auth = request.headers.get("authorization") ?? "";
    const m =
      /^AWS4-HMAC-SHA256 Credential=([^/]+)\/(\d{8})\/([^/]+)\/s3\/aws4_request, SignedHeaders=([^,]+), Signature=([0-9a-f]{64})$/.exec(
        auth,
      );
    if (!m) return "no SigV4 authorization header";
    const [, accessKey, , region, signedHeaders, signature] = m;
    if (accessKey !== ACCESS_KEY) return "InvalidAccessKeyId";
    const contentSha = request.headers.get("x-amz-content-sha256");
    if (contentSha !== hash("sha256", body, "hex")) return "XAmzContentSHA256Mismatch";
    const amzDate = request.headers.get("x-amz-date");
    const date = new Date(
      `${amzDate.slice(0, 4)}-${amzDate.slice(4, 6)}-${amzDate.slice(6, 8)}T${amzDate.slice(9, 11)}:${amzDate.slice(11, 13)}:${amzDate.slice(13, 15)}Z`,
    );
    if (Math.abs(date.getTime() - (Date.now() + skewMs)) > 15 * 60 * 1000)
      return "RequestTimeTooSkewed";
    const url = new URL(request.url);
    const query = [...url.searchParams];
    const headers = signedHeaders
      .split(";")
      .filter(
        (n) => n !== "x-amz-date" && n !== "x-amz-content-sha256" && n !== "x-amz-security-token",
      )
      .map((n) => [n, request.headers.get(n) ?? ""]);
    const expect = signHeaders(
      { method: request.method, canonicalUri: path, query, headers },
      {
        accessKeyId: ACCESS_KEY,
        secretAccessKey: SECRET_KEY,
        sessionToken: request.headers.get("x-amz-security-token") ?? undefined,
      },
      { region, service: "s3", date },
      { payloadHash: contentSha, contentSha256Header: true },
    );
    return expect.signature === signature ? null : "SignatureDoesNotMatch";
  }

  async function handle(request) {
    const url = new URL(request.url);
    // The raw path, as sent — URL would normalize it, which is the point.
    const path = url.pathname;
    const body = new Uint8Array(await request.arrayBuffer());
    const entry = {
      method: request.method,
      path,
      query: Object.fromEntries(url.searchParams),
      headers: Object.fromEntries(request.headers),
      body,
    };
    log.push(entry);

    const fault = faults.findIndex((f) => f.match(entry));
    if (fault >= 0) {
      const [{ response }] = faults.splice(fault, 1);
      return typeof response === "function" ? response(entry) : response.clone();
    }

    if (url.searchParams.has("X-Amz-Signature")) return presigned(request, url, path, body);
    const problem = verify(request, path, body);
    if (problem === "RequestTimeTooSkewed") {
      return new Response(
        "<Error><Code>RequestTimeTooSkewed</Code><Message>skewed</Message></Error>",
        {
          status: 403,
          headers: { date: new Date(Date.now() + skewMs).toUTCString() },
        },
      );
    }
    if (problem) return errorXml(problem.includes(" ") ? "AccessDenied" : problem, problem, 403);

    const [, bucketName, ...rest] = path.split("/");
    const bucket = buckets.get(decodeURIComponent(bucketName));
    if (!bucket) return errorXml("NoSuchBucket", "no bucket", 404);
    const key = rest.length ? rest.map(decodeURIComponent).join("/") : undefined;
    const q = url.searchParams;

    if (key === undefined) {
      if (request.method === "GET" && q.get("list-type") === "2")
        return list(bucket, q, decodeURIComponent(bucketName));
      if (request.method === "POST" && q.has("delete")) return deleteMany(bucket, request, body);
      return errorXml("NotImplemented", "bucket op", 501);
    }

    if (request.method === "POST" && q.has("uploads")) {
      const id = `upload-${nextUpload++}`;
      uploads.set(id, {
        bucket,
        key,
        headers: Object.fromEntries(request.headers),
        parts: new Map(),
      });
      return xml(
        `<InitiateMultipartUploadResult><Bucket>b</Bucket><Key>${esc(key)}</Key><UploadId>${id}</UploadId></InitiateMultipartUploadResult>`,
      );
    }
    if (q.has("uploadId")) {
      const upload = uploads.get(q.get("uploadId"));
      if (!upload) return errorXml("NoSuchUpload", "no upload", 404);
      if (request.method === "PUT") {
        const etag = etagOf(body);
        upload.parts.set(Number(q.get("partNumber")), { bytes: body, etag });
        return new Response(null, { headers: { etag } });
      }
      if (request.method === "DELETE") {
        uploads.delete(q.get("uploadId"));
        return new Response(null, { status: 204 });
      }
      if (request.method === "POST") {
        const text = new TextDecoder().decode(body);
        const numbers = [...text.matchAll(/<PartNumber>(\d+)<\/PartNumber><ETag>([^<]+)<\/ETag>/g)];
        const chunks = [];
        for (const [, n, etag] of numbers) {
          const part = upload.parts.get(Number(n));
          if (!part || part.etag !== etag.replace(/&quot;/g, '"'))
            return errorXml("InvalidPart", `part ${n}`, 400);
          chunks.push(part.bytes);
        }
        const size = chunks.reduce((a, c) => a + c.length, 0);
        const all = new Uint8Array(size);
        let at = 0;
        for (const c of chunks) {
          all.set(c, at);
          at += c.length;
        }
        const etag = `"${hash("md5", all, "hex")}-${chunks.length}"`;
        upload.bucket.set(upload.key, { bytes: all, headers: upload.headers, etag });
        uploads.delete(q.get("uploadId"));
        return xml(
          `<CompleteMultipartUploadResult><Key>${esc(upload.key)}</Key><ETag>${esc(etag)}</ETag></CompleteMultipartUploadResult>`,
        );
      }
    }

    const object = bucket.get(key);
    switch (request.method) {
      case "PUT": {
        const source = request.headers.get("x-amz-copy-source");
        if (source) {
          const [srcBucket, ...srcKey] = source.split("/");
          const from = buckets
            .get(decodeURIComponent(srcBucket))
            ?.get(srcKey.map(decodeURIComponent).join("/"));
          if (!from) return errorXml("NoSuchKey", "copy source", 404);
          const headers =
            request.headers.get("x-amz-metadata-directive") === "REPLACE"
              ? Object.fromEntries(request.headers)
              : from.headers;
          bucket.set(key, { bytes: from.bytes, headers, etag: from.etag });
          return xml(
            `<CopyObjectResult><LastModified>2026-11-11T00:00:00.000Z</LastModified><ETag>${esc(from.etag)}</ETag></CopyObjectResult>`,
          );
        }
        if (request.headers.get("if-none-match") === "*" && object)
          return errorXml("PreconditionFailed", "exists", 412);
        if (request.headers.has("if-match") && request.headers.get("if-match") !== object?.etag) {
          return errorXml("PreconditionFailed", "etag", 412);
        }
        for (const alg of ["crc32", "crc32c", "sha1", "sha256"]) {
          const sent = request.headers.get(`x-amz-checksum-${alg}`);
          if (sent && sent !== hash(alg, body, "base64")) return errorXml("BadDigest", alg, 400);
        }
        const etag = etagOf(body);
        bucket.set(key, { bytes: body, headers: Object.fromEntries(request.headers), etag });
        return new Response(null, { headers: { etag } });
      }
      case "GET":
      case "HEAD": {
        if (!object)
          return request.method === "HEAD"
            ? new Response(null, { status: 404 })
            : errorXml("NoSuchKey", "The specified key does not exist.", 404);
        if (request.headers.has("if-match") && request.headers.get("if-match") !== object.etag) {
          return request.method === "HEAD"
            ? new Response(null, { status: 412 })
            : errorXml("PreconditionFailed", "etag", 412);
        }
        if (request.headers.get("if-none-match") === object.etag)
          return new Response(null, { status: 304 });
        const headers = {
          etag: object.etag,
          "content-type": object.headers["content-type"] ?? "binary/octet-stream",
          "last-modified": "Wed, 11 Nov 2026 00:00:00 GMT",
        };
        for (const [name, value] of Object.entries(object.headers)) {
          if (
            name.startsWith("x-amz-meta-") ||
            name === "content-encoding" ||
            name === "cache-control"
          )
            headers[name] = value;
        }
        let bytes = object.bytes;
        let status = 200;
        const range = request.headers.get("range");
        if (range) {
          const [, a, b] = /^bytes=(-?\d*)-?(\d*)$/.exec(range);
          let start;
          let end;
          if (a.startsWith("-")) {
            start = Math.max(0, bytes.length + Number(a));
            end = bytes.length - 1;
          } else {
            start = Number(a);
            end = b === "" ? bytes.length - 1 : Math.min(Number(b), bytes.length - 1);
          }
          if (start >= bytes.length) return errorXml("InvalidRange", "range", 416);
          headers["content-range"] = `bytes ${start}-${end}/${bytes.length}`;
          bytes = bytes.slice(start, end + 1);
          status = 206;
        }
        // runtime:http drops a Content-Length set on a body-less HEAD answer,
        // so the body is given and the server sends its length, not its bytes.
        return new Response(bytes, { status, headers });
      }
      case "DELETE":
        bucket.delete(key);
        return new Response(null, { status: 204 });
    }
    return errorXml("NotImplemented", request.method, 501);
  }

  function presigned(request, url, path, body) {
    const q = url.searchParams;
    const [, bucketName, ...rest] = path.split("/");
    const key = rest.map(decodeURIComponent).join("/");
    const expires = Number(q.get("X-Amz-Expires"));
    const d = q.get("X-Amz-Date");
    const date = new Date(
      `${d.slice(0, 4)}-${d.slice(4, 6)}-${d.slice(6, 8)}T${d.slice(9, 11)}:${d.slice(11, 13)}:${d.slice(13, 15)}Z`,
    );
    if (Date.now() > date.getTime() + expires * 1000)
      return errorXml("AccessDenied", "Request has expired", 403);
    const object = buckets.get(decodeURIComponent(bucketName))?.get(key);
    if (request.method === "GET")
      return object
        ? new Response(object.bytes, { headers: { etag: object.etag } })
        : errorXml("NoSuchKey", "missing", 404);
    if (request.method === "PUT") {
      buckets.get(decodeURIComponent(bucketName)).set(key, {
        bytes: body,
        headers: Object.fromEntries(request.headers),
        etag: etagOf(body),
      });
      return new Response(null, { headers: { etag: etagOf(body) } });
    }
    return errorXml("NotImplemented", "presigned", 501);
  }

  function list(bucket, q, name) {
    const prefix = q.get("prefix") ?? "";
    const delimiter = q.get("delimiter") ?? "";
    const max = Number(q.get("max-keys") ?? 1000);
    const after = q.get("continuation-token") ?? q.get("start-after") ?? "";
    const encode = q.get("encoding-type") === "url" ? urlEncodeKey : esc;
    const keys = [...bucket.keys()].filter((k) => k.startsWith(prefix) && k > after).sort();
    const contents = [];
    const prefixes = new Set();
    let truncated = false;
    let last = "";
    for (const k of keys) {
      if (contents.length + prefixes.size >= max) {
        truncated = true;
        break;
      }
      const restKey = k.slice(prefix.length);
      const at = delimiter ? restKey.indexOf(delimiter) : -1;
      if (at >= 0) prefixes.add(prefix + restKey.slice(0, at + delimiter.length));
      else contents.push(k);
      last = k;
    }
    const items = contents
      .map(
        (k) =>
          `<Contents><Key>${encode(k)}</Key><LastModified>2026-11-11T00:00:00.000Z</LastModified><ETag>${esc(bucket.get(k).etag)}</ETag><Size>${bucket.get(k).bytes.length}</Size><StorageClass>STANDARD</StorageClass></Contents>`,
      )
      .join("");
    const common = [...prefixes]
      .map((p) => `<CommonPrefixes><Prefix>${encode(p)}</Prefix></CommonPrefixes>`)
      .join("");
    return xml(
      `<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Name>${name}</Name><Prefix>${encode(prefix)}</Prefix><KeyCount>${contents.length + prefixes.size}</KeyCount><MaxKeys>${max}</MaxKeys>${delimiter ? `<Delimiter>${encode(delimiter)}</Delimiter>` : ""}${q.get("encoding-type") === "url" ? "<EncodingType>url</EncodingType>" : ""}<IsTruncated>${truncated}</IsTruncated>${truncated ? `<NextContinuationToken>${esc(last)}</NextContinuationToken>` : ""}${items}${common}</ListBucketResult>`,
    );
  }

  function deleteMany(bucket, request, body) {
    if (request.headers.get("content-md5") !== hash("md5", body, "base64"))
      return errorXml("InvalidDigest", "md5", 400);
    const text = new TextDecoder().decode(body);
    const errors = [];
    for (const [, raw] of text.matchAll(/<Key>([^<]*)<\/Key>/g)) {
      const key = raw.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
      if (key.startsWith("locked/"))
        errors.push(
          `<Error><Key>${esc(key)}</Key><Code>AccessDenied</Code><Message>locked</Message></Error>`,
        );
      else bucket.delete(key);
    }
    return xml(`<DeleteResult>${errors.join("")}</DeleteResult>`);
  }

  const server = serve({ port: 0, hostname: "127.0.0.1" }, handle);
  const { port } = await server.addr;
  return {
    endpoint: `http://127.0.0.1:${port}`,
    buckets,
    uploads,
    log,
    fail(match, response) {
      faults.push({ match, response });
    },
    setSkew(ms) {
      skewMs = ms;
    },
    stop: () => server.stop(),
  };
}
