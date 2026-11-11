// The client against the fake S3: every request's signature is checked by the
// fake from what arrived, so these pass only if what was signed is what was sent.

import { backoff } from "../../dist/core.js";
import { S3Client, S3Error } from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";
import { ACCESS_KEY, errorXml, SECRET_KEY, startFake } from "./fake-s3.mjs";

backoff.base = 1;
backoff.throttledBase = 1;

const fake = await startFake();
const client = new S3Client({
  endpoint: fake.endpoint,
  bucket: "bucket",
  accessKeyId: ACCESS_KEY,
  secretAccessKey: SECRET_KEY,
});
const last = () => fake.log[fake.log.length - 1];

async function rejects(fn, code, what) {
  try {
    await fn();
    ok(false, `${what} — nothing was thrown`);
  } catch (error) {
    is(error?.code, code, what);
    return error;
  }
}

// -- write and read ---------------------------------------------------------

{
  const result = await client.write("hello.txt", "hello, s3");
  ok(result.etag.startsWith('"'), "write returns the quoted ETag");
  is(result.size, 9, "and the size");
  is(last().headers["content-type"], "text/plain;charset=utf-8", "a string is text/plain");
  const file = client.file("hello.txt");
  is(await file.text(), "hello, s3", "text() reads it back");
  is(file.name, "hello.txt", "name is the key, as in Bun");
  const stat = await file.stat();
  is(stat.size, 9, "stat size");
  is(stat.etag, result.etag, "stat etag matches the write");
  ok(stat.lastModified instanceof Date, "lastModified is a Date");
  ok(await file.exists(), "exists");
  ok(!(await client.exists("missing.txt")), "a missing key does not exist");
  is(await client.size("hello.txt"), 9, "size()");
}

{
  await client.write("data.json", JSON.stringify({ a: 1 }));
  is(last().headers["content-type"], "application/json", "a .json key is typed by its extension");
  is((await client.file("data.json").json()).a, 1, "json()");
  await client.write("raw.bin", new Uint8Array([0, 1, 2, 255]), { type: "application/x-thing" });
  is(last().headers["content-type"], "application/x-thing", "type wins over the extension");
  is([...(await client.file("raw.bin").bytes())].join(","), "0,1,2,255", "bytes()");
  await client.write("blob.txt", new Blob(["from a blob"], { type: "text/x-blob" }));
  is(last().headers["content-type"], "text/x-blob", "a Blob's type is kept");
  is(await (await client.file("blob.txt").blob()).text(), "from a blob", "blob()");
}

// Keys that need encoding, and keys that are refused.
{
  const keys = [
    "dir/a b+c.txt",
    "über/ü.txt",
    " leading and trailing ",
    "a//double",
    "(x)!*'.txt",
    "100%",
  ];
  for (const key of keys) {
    await client.write(key, key);
    is(await client.file(key).text(), key, `round trip of ${JSON.stringify(key)}`);
  }
  for (const key of ["a/../b", "./x", "a/.", "", "x".repeat(1025)]) {
    await rejects(
      () => client.write(key, "x"),
      "ERR_S3_INVALID_ARGUMENT",
      `refuses ${JSON.stringify(key.slice(0, 20))}`,
    );
  }
}

// -- stored bytes are stored bytes -------------------------------------------

{
  const plain = "compressible ".repeat(200);
  const gz = new Uint8Array(
    await new Response(
      new Blob([plain]).stream().pipeThrough(new CompressionStream("gzip")),
    ).arrayBuffer(),
  );
  await client.write("site/app.js", gz, { contentEncoding: "gzip", type: "text/javascript" });
  const back = await client.file("site/app.js").bytes();
  is(back.length, gz.length, "a gzip-stored object reads back as stored, not decoded");
  is(last().headers["accept-encoding"], "identity", "and the GET asked for no coding");
  const stat = await client.stat("site/app.js");
  is(stat.contentEncoding, "gzip", "stat reports the stored coding");
  const part = await client.file("site/app.js").slice(0, 10).bytes();
  is(
    [...part].join(","),
    [...gz.slice(0, 10)].join(","),
    "a range of a gzip-stored object is its stored bytes",
  );
}

// -- slices ------------------------------------------------------------------

{
  await client.write("digits", "0123456789");
  const f = client.file("digits");
  is(await f.slice(2, 5).text(), "234", "slice(2, 5)");
  is(await f.slice(7).text(), "789", "slice(7)");
  is(await f.slice(-3).text(), "789", "slice(-3) is a suffix");
  is(await f.slice(4, 4).text(), "", "an empty slice sends nothing");
  is(await f.slice(20).text(), "", "a slice past the end is empty, as Blob.slice is");
  is(await f.slice(2, 8).slice(1, 3).text(), "34", "a slice of a slice");
  throws(() => f.slice(0, -1), "a negative end is refused");
}

// -- streams -----------------------------------------------------------------

{
  const chunks = [];
  for await (const chunk of client.file("digits").stream())
    chunks.push(new TextDecoder().decode(chunk));
  is(chunks.join(""), "0123456789", "stream()");
  const source = new ReadableStream({
    start(c) {
      c.enqueue(new TextEncoder().encode("streamed "));
      c.enqueue(new TextEncoder().encode("body"));
      c.close();
    },
  });
  const before = fake.log.length;
  await client.write("streamed.txt", source);
  is(fake.log.length - before, 1, "a small stream is one PUT, not a multipart upload");
  is(await client.file("streamed.txt").text(), "streamed body", "write(ReadableStream)");
  await client.write(
    "from-response.txt",
    new Response("via response", { headers: { "content-type": "text/x-r" } }),
  );
  is(await client.file("from-response.txt").text(), "via response", "write(Response)");
  await client.write("copy-of-digits", client.file("digits"));
  is(await client.file("copy-of-digits").text(), "0123456789", "write(S3File) streams it across");
}

// -- multipart ---------------------------------------------------------------

{
  const MiB = 1024 * 1024;
  const big = new Uint8Array(11 * MiB + 123);
  for (let i = 0; i < big.length; i++) big[i] = (i * 31) & 0xff;
  const before = fake.log.length;
  const result = await client.write("big.bin", new Blob([big]), { checksum: "crc32c" });
  const sent = fake.log.slice(before);
  is(sent.filter((r) => r.method === "PUT").length, 3, "11 MiB in 5 MiB parts is three parts");
  ok(sent[0].query.uploads !== undefined, "it starts with CreateMultipartUpload");
  is(
    sent[0].headers["x-amz-checksum-algorithm"],
    "CRC32C",
    "the checksum algorithm is declared up front",
  );
  ok(
    sent.filter((r) => r.method === "PUT").every((r) => r.headers["x-amz-checksum-crc32c"]),
    "and every part carries its checksum",
  );
  ok(result.etag.endsWith('-3"'), "the ETag is a multipart one");
  is(result.size, big.length, "write reports the size");
  const back = await client.file("big.bin").bytes();
  ok(
    back.length === big.length && back.every((b, i) => b === big[i]),
    "the parts reassemble in order",
  );
  is(fake.uploads.size, 0, "no upload is left open");
}

{
  const writer = client.file("written.txt").writer();
  await writer.write("a");
  await writer.write(new Uint8Array([98]));
  await writer.write(new TextEncoder().encode("c").buffer);
  const result = await writer.end();
  is(result.size, 3, "writer size");
  is(await client.file("written.txt").text(), "abc", "writer() with small writes is one PUT");
  await throws(() => writer.write("more"), "a writer that ended refuses writes");
}

{
  // A part that keeps failing fails the upload, and the upload is aborted.
  const MiB = 1024 * 1024;
  fake.fail(
    (r) => r.method === "PUT" && r.query.partNumber === "2",
    errorXml("InternalError", "boom", 500),
  );
  fake.fail(
    (r) => r.method === "PUT" && r.query.partNumber === "2",
    errorXml("InternalError", "boom", 500),
  );
  const writer = client.file("doomed.bin").writer({ retry: 1 });
  let failed;
  try {
    await writer.write(new Uint8Array(11 * MiB));
    await writer.end();
  } catch (error) {
    failed = error;
  }
  is(failed?.code, "InternalError", "the part's error is the upload's error");
  is(fake.uploads.size, 0, "the multipart upload was aborted");
  ok(!(await client.exists("doomed.bin")), "and nothing was written");
}

{
  // CompleteMultipartUpload can answer 200 with an error in the body.
  const MiB = 1024 * 1024;
  fake.fail(
    (r) => r.method === "POST" && r.query.uploadId !== undefined,
    new Response("<Error><Code>InternalError</Code><Message>try again</Message></Error>", {
      status: 200,
    }),
  );
  const result = await client.write("retried-complete.bin", new Blob([new Uint8Array(6 * MiB)]));
  ok(result.etag.endsWith('-2"'), "a 200 with an <Error> body is retried, not taken as success");
}

await rejects(
  () => client.write("x", "y", { partSize: 1024 }),
  "ERR_S3_INVALID_ARGUMENT",
  "a part smaller than 5 MiB is refused",
);

// -- conditional requests ----------------------------------------------------

{
  const first = await client.write("cond.txt", "v1", { ifNoneMatch: "*" });
  await rejects(
    () => client.write("cond.txt", "v2", { ifNoneMatch: "*" }),
    "PreconditionFailed",
    "ifNoneMatch: * refuses an overwrite",
  );
  await client.write("cond.txt", "v2", { ifMatch: first.etag });
  is(await client.file("cond.txt").text(), "v2", "ifMatch with the current ETag writes");
  await rejects(
    () => client.file("cond.txt", { ifMatch: first.etag }).text(),
    "PreconditionFailed",
    "a read with a stale ifMatch fails",
  );
  const stat = await client.stat("cond.txt");
  await rejects(
    () => client.file("cond.txt", { ifNoneMatch: stat.etag }).text(),
    "NotModified",
    "ifNoneMatch on the current ETag is NotModified",
  );
}

// -- metadata and options as headers ----------------------------------------

{
  await client.write("meta.txt", "m", {
    metadata: { owner: "ops", Purpose: "test" },
    cacheControl: "max-age=60",
    acl: "public-read",
    storageClass: "STANDARD_IA",
    contentDisposition: 'attachment; filename="m.txt"',
  });
  const h = last().headers;
  is(h["x-amz-meta-owner"], "ops", "metadata is x-amz-meta-*");
  is(h["x-amz-meta-purpose"], "test", "metadata names are lower-cased");
  is(h["x-amz-acl"], "public-read", "acl");
  is(h["x-amz-storage-class"], "STANDARD_IA", "storageClass");
  const stat = await client.stat("meta.txt");
  is(stat.metadata.owner, "ops", "stat reads metadata back");
  is(stat.cacheControl, "max-age=60", "and cache-control");
  await rejects(
    () => client.write("m", "x", { metadata: { "bad name": "x" } }),
    "ERR_S3_INVALID_ARGUMENT",
    "a metadata name that is not a token",
  );
  await rejects(
    () => client.write("m", "x", { metadata: { n: "ünicode" } }),
    "ERR_S3_INVALID_ARGUMENT",
    "a non-ASCII metadata value",
  );
  await rejects(
    () => client.write("m", "x", { sseCustomerKey: new Uint8Array(32) }),
    "ERR_S3_INVALID_ARGUMENT",
    "an SSE-C key is never sent over plain HTTP",
  );
}

// -- list --------------------------------------------------------------------

{
  for (const key of [
    "l/a.txt",
    "l/b.txt",
    "l/sub/c.txt",
    "l/sub/d.txt",
    "l/ spaced .txt",
    "l/plus+sign",
  ])
    await client.write(key, key);
  const page = await client.list({ prefix: "l/", delimiter: "/" });
  is(
    page.contents
      .map((c) => c.key)
      .sort()
      .join("|"),
    "l/ spaced .txt|l/a.txt|l/b.txt|l/plus+sign",
    "list keys, decoded (spaces and + survive)",
  );
  is(
    page.commonPrefixes.map((p) => p.prefix).join("|"),
    "l/sub/",
    "delimiter groups into commonPrefixes",
  );
  is(page.prefix, "l/", "prefix is decoded");
  is(last().query["encoding-type"], "url", "list asks for URL-encoded keys");
  ok(page.contents[0].lastModified instanceof Date, "lastModified is a Date");
  ok(
    page.contents.every((c) => typeof c.size === "number"),
    "sizes are numbers",
  );

  const paged = await client.list({ prefix: "l/", maxKeys: 2 });
  ok(
    paged.isTruncated && paged.nextContinuationToken,
    "maxKeys truncates with a continuation token",
  );
  const all = [];
  for await (const item of client.listAll({ prefix: "l/" })) all.push(item.key);
  is(all.length, 6, "listAll follows continuation tokens");
  await rejects(
    () => client.list({ maxKeys: 0 }),
    "ERR_S3_INVALID_ARGUMENT",
    "maxKeys 0 is refused",
  );
}

// -- copy and delete ---------------------------------------------------------

{
  const copied = await client.copy("hello.txt", "copies/hello copy.txt");
  is(
    last().headers["x-amz-copy-source"],
    "bucket/hello.txt",
    "the copy source is bucket/key, encoded",
  );
  ok(copied.etag.startsWith('"'), "copy returns the ETag");
  is(await client.file("copies/hello copy.txt").text(), "hello, s3", "copy");
  fake.fail(
    (r) => r.headers["x-amz-copy-source"] !== undefined,
    new Response("<Error><Code>SlowDown</Code><Message>later</Message></Error>", { status: 200 }),
  );
  await client.copy("hello.txt", "copies/retried.txt");
  is(
    await client.file("copies/retried.txt").text(),
    "hello, s3",
    "a CopyObject 200-with-Error is retried",
  );
  await client.copy("meta.txt", "copies/replaced.txt", { metadata: { owner: "dev" } });
  is(
    (await client.stat("copies/replaced.txt")).metadata.owner,
    "dev",
    "metadata on copy replaces it",
  );

  await client.delete("copies/hello copy.txt");
  ok(!(await client.exists("copies/hello copy.txt")), "delete");
  await client.delete("never-existed");
  ok(true, "deleting a missing key succeeds");

  const many = Array.from({ length: 1203 }, (_, i) => `bulk/${i}`);
  for (const key of many.slice(0, 5)) await client.write(key, "x");
  await client.write("locked/one", "x");
  const before = fake.log.length;
  const { errors } = await client.deleteMany([...many, "locked/one", "a <&> b"]);
  is(fake.log.length - before, 2, "1205 keys are two DeleteObjects requests");
  is(errors.length, 1, "deleteMany returns what could not be deleted");
  is(errors[0]?.key, "locked/one", "with its key");
  is(errors[0]?.code, "AccessDenied", "and S3's code");
  ok(!(await client.exists("bulk/0")), "the rest are gone");
}

// -- presign -----------------------------------------------------------------

{
  const url = await client.presign("hello.txt", { expiresIn: 60 });
  const parsed = new URL(url);
  is(parsed.searchParams.get("X-Amz-Expires"), "60", "expiresIn");
  is(parsed.searchParams.get("X-Amz-SignedHeaders"), "host", "a GET link signs only the host");
  is(await (await fetch(url)).text(), "hello, s3", "a presigned GET fetches without credentials");
  const put = await client.file("uploaded.txt").presign({ method: "PUT", expiresIn: 60 });
  await fetch(put, { method: "PUT", body: "via link" });
  is(await client.file("uploaded.txt").text(), "via link", "a presigned PUT uploads");
  is(
    new URL(await client.presign("x")).searchParams.get("X-Amz-Expires"),
    "86400",
    "the default is a day, as in Bun",
  );
  await rejects(
    () => client.presign("x", { expiresIn: 604801 }),
    "ERR_S3_INVALID_ARGUMENT",
    "more than 7 days is refused",
  );
  const disp = new URL(
    await client.presign("x", { contentDisposition: 'attachment; filename="a b.txt"' }),
  );
  is(
    disp.searchParams.get("response-content-disposition"),
    'attachment; filename="a b.txt"',
    "contentDisposition rides the query",
  );
}

// -- errors and retries ------------------------------------------------------

{
  const error = await rejects(
    () => client.file("missing.txt").text(),
    "NoSuchKey",
    "a missing key is NoSuchKey",
  );
  ok(error instanceof S3Error, "an S3Error");
  is(error.status, 404, "with the status");
  is(error.requestId, "req-1", "and the request id");
  is(error.key, "missing.txt", "and the key");
  ok(
    error.message.includes("NoSuchKey") && error.message.includes("bucket/missing.txt"),
    "the message names the code and the object",
  );

  const head = await rejects(
    () => client.stat("missing.txt"),
    "NotFound",
    "a HEAD has no body, so its code comes from the status",
  );
  is(head.status, 404, "HEAD status");

  fake.fail(
    (r) => r.path === "/bucket/flaky.txt",
    errorXml("SlowDown", "Please reduce your request rate.", 503),
  );
  fake.fail((r) => r.path === "/bucket/flaky.txt", errorXml("InternalError", "oops", 500));
  await client.write("flaky.txt", "made it");
  is(await client.file("flaky.txt").text(), "made it", "SlowDown and InternalError are retried");

  fake.fail((r) => r.path === "/bucket/denied.txt", errorXml("AccessDenied", "no", 403));
  const before = fake.log.length;
  await rejects(
    () => client.write("denied.txt", "x"),
    "AccessDenied",
    "AccessDenied is not retried",
  );
  is(fake.log.length - before, 1, "one attempt");

  const quick = new S3Client({
    endpoint: fake.endpoint,
    bucket: "bucket",
    accessKeyId: ACCESS_KEY,
    secretAccessKey: SECRET_KEY,
    retry: 0,
  });
  fake.fail((r) => r.path === "/bucket/once.txt", errorXml("InternalError", "x", 500));
  await rejects(() => quick.write("once.txt", "x"), "InternalError", "retry: 0 tries once");

  fake.fail(
    (r) => r.path === "/bucket/moved.txt",
    new Response("<Error><Code>PermanentRedirect</Code><Message>wrong endpoint</Message></Error>", {
      status: 301,
      headers: { "x-amz-bucket-region": "eu-west-1" },
    }),
  );
  const moved = await rejects(
    () => client.write("moved.txt", "x"),
    "PermanentRedirect",
    "a redirect is reported, not followed",
  );
  is(moved.region, "eu-west-1", "with the bucket's region");
  ok(moved.message.includes('region: "eu-west-1"'), "and the fix in the message");

  const wrong = new S3Client({
    endpoint: fake.endpoint,
    bucket: "bucket",
    accessKeyId: ACCESS_KEY,
    secretAccessKey: "not the secret",
  });
  await rejects(
    () => wrong.write("x", "y"),
    "SignatureDoesNotMatch",
    "a wrong secret is S3's SignatureDoesNotMatch",
  );

  const unreachable = new S3Client({
    endpoint: "http://127.0.0.1:1",
    bucket: "bucket",
    accessKeyId: "a",
    secretAccessKey: "b",
    retry: 1,
  });
  await rejects(
    () => unreachable.write("x", "y"),
    "ERR_S3_NETWORK",
    "a connection failure is ERR_S3_NETWORK",
  );

  const controller = new AbortController();
  controller.abort(new Error("stop"));
  try {
    await client.write("aborted.txt", "x", { signal: controller.signal });
    ok(false, "an aborted signal stops the request");
  } catch (error) {
    is(error?.message, "stop", "an aborted request rejects with the signal's reason");
  }
}

{
  // The server's clock is ahead: the first request is refused, the client
  // learns the offset from Date and the retry is signed in server time.
  fake.setSkew(60 * 60 * 1000);
  await client.write("skewed.txt", "on time");
  fake.setSkew(0);
  ok(
    fake.log.some((r) => r.path === "/bucket/skewed.txt"),
    "a RequestTimeTooSkewed is corrected from the server's Date",
  );
}

// -- credentials -------------------------------------------------------------

{
  await rejects(
    () => new S3Client({ endpoint: fake.endpoint, bucket: "bucket" }).write("x", "y"),
    "ERR_S3_CREDENTIALS",
    "no credentials is ERR_S3_CREDENTIALS",
  );
  let calls = 0;
  const provided = new S3Client({
    endpoint: fake.endpoint,
    bucket: "bucket",
    credentials: async () => {
      calls++;
      return {
        accessKeyId: ACCESS_KEY,
        secretAccessKey: SECRET_KEY,
        expiration: new Date(Date.now() + 60 * 60 * 1000),
      };
    },
  });
  await Promise.all([
    provided.write("p1", "1"),
    provided.write("p2", "2"),
    provided.write("p3", "3"),
  ]);
  is(calls, 1, "a provider is asked once, and concurrent requests share the answer");
  let soon = 0;
  const expiring = new S3Client({
    endpoint: fake.endpoint,
    bucket: "bucket",
    credentials: () => {
      soon++;
      return {
        accessKeyId: ACCESS_KEY,
        secretAccessKey: SECRET_KEY,
        expiration: new Date(Date.now() + 60 * 1000),
      };
    },
  });
  await expiring.write("e1", "1");
  await expiring.write("e2", "2");
  is(soon, 2, "credentials within five minutes of expiring are refreshed");
  const failing = new S3Client({
    endpoint: fake.endpoint,
    bucket: "bucket",
    credentials: () => Promise.reject(new Error("metadata service down")),
  });
  const failed = await rejects(
    () => failing.write("x", "y"),
    "ERR_S3_CREDENTIALS",
    "a failing provider is ERR_S3_CREDENTIALS",
  );
  ok(failed.message.includes("metadata service down"), "naming the provider's error");
}

// -- configuration -----------------------------------------------------------

{
  throws(() => new S3Client({ endpoint: "ftp://x" }), "a non-HTTP endpoint is refused");
  throws(
    () => new S3Client({ endpoint: "https://user:pw@host" }),
    "credentials in the endpoint are refused",
  );
  throws(() => new S3Client({ region: "US East" }), "a region that is not a region name");
  throws(() => new S3Client({ bucket: "a/b" }), "a bucket with a slash");
  throws(() => client.file("x", { bucket: ".." }), "a bucket named ..");
  throws(() => new S3Client({ queueSize: 0 }), "queueSize 0");
  throws(() => client.file("x").slice(0, 1).writer(), "a slice cannot be written");

  const aws = new S3Client({
    bucket: "my-bucket",
    region: "eu-west-1",
    accessKeyId: "a",
    secretAccessKey: "b",
  });
  is(
    new URL(await aws.presign("k/v.txt")).origin + new URL(await aws.presign("k/v.txt")).pathname,
    "https://s3.eu-west-1.amazonaws.com/my-bucket/k/v.txt",
    "AWS default: path-style on the regional endpoint",
  );
  const vhost = new S3Client({
    bucket: "my-bucket",
    region: "eu-west-1",
    virtualHostedStyle: true,
    accessKeyId: "a",
    secretAccessKey: "b",
  });
  is(
    new URL(await vhost.presign("k")).host,
    "my-bucket.s3.eu-west-1.amazonaws.com",
    "virtualHostedStyle puts the bucket in the host",
  );
  const own = new S3Client({
    endpoint: "https://media.example.com",
    virtualHostedStyle: true,
    bucket: "ignored",
    accessKeyId: "a",
    secretAccessKey: "b",
  });
  is(
    new URL(await own.presign("k")).href.split("?")[0],
    "https://media.example.com/k",
    "with an endpoint, virtualHostedStyle takes it as the bucket's own host",
  );
  const prefixed = new S3Client({
    endpoint: "https://gw.example.com/s3/",
    bucket: "b",
    accessKeyId: "a",
    secretAccessKey: "b",
  });
  is(new URL(await prefixed.presign("k")).pathname, "/s3/b/k", "an endpoint's path is kept");
}

// The credentials are not on the client for console.log to find.
ok(
  !JSON.stringify(client).includes(SECRET_KEY) && !String(Object.keys(client)).includes("secret"),
  "the secret is not enumerable on the client",
);

await fake.stop();
if (report("client") > 0) throw new Error("client failed");
