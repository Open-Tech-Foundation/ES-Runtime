// End-to-end: a real S3-compatible server, nothing scripted.
//
// Skips itself when S3_E2E_ENDPOINT is not set, so `test/run.sh` stays runnable
// without Docker. Set it with `eval "$(test/s3-server.sh)"`. Works against AWS
// too: point S3_E2E_ENDPOINT at https://s3.<region>.amazonaws.com and set
// S3_E2E_REGION; it creates and removes its own bucket.
import { env, exit, unmask } from "runtime:process";
import { headSource, multipartCopy } from "../dist/copy.js";
import { Core } from "../dist/core.js";
import { CredentialCache } from "../dist/credentials.js";
import { S3Client } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const endpoint = env.S3_E2E_ENDPOINT ?? "";
if (endpoint === "") {
  console.log('skip e2e (set S3_E2E_ENDPOINT, e.g. eval "$(test/s3-server.sh)")');
  exit(0);
}
const region = env.S3_E2E_REGION ?? "us-east-1";
const accessKeyId = unmask(env.S3_E2E_ACCESS_KEY);
const secretAccessKey = unmask(env.S3_E2E_SECRET_KEY);
const bucket = `esrun-e2e-${Math.random().toString(36).slice(2, 10)}`;
const MiB = 1024 * 1024;

// Bucket administration is not this package's job; the test does it with the
// same signed core the client uses.
const core = new Core({
  endpoint: new URL(endpoint),
  region,
  virtualHostedStyle: false,
  retry: 2,
  credentials: new CredentialCache(() => ({ accessKeyId, secretAccessKey })),
});
const body =
  region === "us-east-1"
    ? undefined
    : new TextEncoder().encode(
        `<CreateBucketConfiguration><LocationConstraint>${region}</LocationConstraint></CreateBucketConfiguration>`,
      );
await core.send({ method: "PUT", bucket, body });

const s3 = new S3Client({ endpoint, region, bucket, accessKeyId, secretAccessKey });

try {
  // -- basics ---------------------------------------------------------------
  const written = await s3.write("hello.txt", "hello from esrun");
  ok(written.etag.length > 2, "write returns an ETag");
  is(await s3.file("hello.txt").text(), "hello from esrun", "read it back");
  const stat = await s3.stat("hello.txt");
  is(stat.size, 16, "stat size");
  is(stat.etag, written.etag, "stat ETag matches");
  is(stat.type, "text/plain;charset=utf-8", "the type is what was sent");
  ok(await s3.exists("hello.txt"), "exists");
  ok(!(await s3.exists("nope.txt")), "missing does not exist");
  try {
    await s3.file("nope.txt").text();
    ok(false, "reading a missing key throws");
  } catch (error) {
    is(error.code, "NoSuchKey", "a missing key is NoSuchKey");
    is(error.status, 404, "with 404");
  }

  // -- the bytes stored are the bytes read (D210) -----------------------------
  const plain = "compress me ".repeat(500);
  const gz = new Uint8Array(
    await new Response(
      new Blob([plain]).stream().pipeThrough(new CompressionStream("gzip")),
    ).arrayBuffer(),
  );
  await s3.write("site/app.js.gz", gz, { contentEncoding: "gzip", type: "text/javascript" });
  const back = await s3.file("site/app.js.gz").bytes();
  ok(
    back.length === gz.length && back.every((b, i) => b === gz[i]),
    "a gzip-stored object reads back as stored",
  );
  const head = await s3.file("site/app.js.gz").slice(0, 16).bytes();
  ok(
    head.every((b, i) => b === gz[i]),
    "and so does a range of it",
  );

  // -- ranges ---------------------------------------------------------------
  await s3.write("digits", "0123456789");
  is(await s3.file("digits").slice(3, 6).text(), "345", "slice");
  is(await s3.file("digits").slice(-2).text(), "89", "suffix slice");
  is(await s3.file("digits").slice(50).text(), "", "a slice past the end is empty");

  // -- keys -----------------------------------------------------------------
  const keys = [
    "k/a b.txt",
    "k/plus+sign",
    "k/über.txt",
    "k/ edge spaces ",
    "k/(paren)!*'",
    "k/percent%41",
  ];
  for (const key of keys) await s3.write(key, key);
  for (const key of keys)
    is(await s3.file(key).text(), key, `key ${JSON.stringify(key)} round-trips`);
  const listed = [];
  for await (const item of s3.listAll({ prefix: "k/" })) listed.push(item.key);
  is(listed.sort().join("|"), [...keys].sort().join("|"), "list returns every key exactly");
  const page = await s3.list({ prefix: "k/", maxKeys: 3 });
  ok(page.isTruncated && page.contents.length === 3, "maxKeys pages");
  await s3.write("dir/x/1", "1");
  await s3.write("dir/y/2", "2");
  await s3.write("dir/top", "t");
  const dir = await s3.list({ prefix: "dir/", delimiter: "/" });
  is(
    dir.commonPrefixes
      .map((p) => p.prefix)
      .sort()
      .join(","),
    "dir/x/,dir/y/",
    "delimiter groups",
  );
  is(dir.contents.map((c) => c.key).join(","), "dir/top", "and leaves the top level");

  // -- metadata -------------------------------------------------------------
  await s3.write("meta.txt", "m", {
    metadata: { owner: "e2e" },
    cacheControl: "no-cache",
    contentDisposition: "inline",
  });
  const meta = await s3.stat("meta.txt");
  is(meta.metadata.owner, "e2e", "metadata round-trips");
  is(meta.cacheControl, "no-cache", "cache-control round-trips");

  // -- checksums ------------------------------------------------------------
  for (const checksum of ["crc32", "crc32c", "sha1", "sha256"]) {
    await s3.write(`sum/${checksum}`, `checked by ${checksum}`, { checksum });
    is(
      await s3.file(`sum/${checksum}`).text(),
      `checked by ${checksum}`,
      `a ${checksum} checksum is accepted`,
    );
  }

  // -- multipart ------------------------------------------------------------
  const big = new Uint8Array(12 * MiB + 7);
  for (let i = 0; i < big.length; i++) big[i] = (i * 7 + (i >> 13)) & 0xff;
  const multipart = await s3.write("big.bin", new Blob([big]), { checksum: "crc32", queueSize: 2 });
  is(multipart.size, big.length, "multipart size");
  const bigBack = await s3.file("big.bin").bytes();
  ok(
    bigBack.length === big.length && bigBack.every((b, i) => b === big[i]),
    "a multipart object reassembles",
  );
  is((await s3.stat("big.bin")).size, big.length, "and stats at its full size");
  const streamed = new ReadableStream({
    pull(controller) {
      controller.enqueue(new Uint8Array(1 * MiB).fill(controller.desiredSize === null ? 0 : 1));
      if (++this.n >= 7) controller.close();
    },
    n: 0,
  });
  is(
    (await s3.write("streamed.bin", streamed)).size,
    7 * MiB,
    "a stream of unknown length uploads in parts",
  );
  is((await s3.stat("streamed.bin")).size, 7 * MiB, "and is whole");

  const writer = s3.file("abandoned.bin").writer();
  await writer.write(new Uint8Array(6 * MiB));
  await writer.abort();
  ok(!(await s3.exists("abandoned.bin")), "an aborted writer leaves no object");

  // -- conditional writes ---------------------------------------------------
  const first = await s3.write("cond.txt", "one", { ifNoneMatch: "*" });
  try {
    await s3.write("cond.txt", "two", { ifNoneMatch: "*" });
    ok(false, "ifNoneMatch: * refuses an overwrite");
  } catch (error) {
    is(error.code, "PreconditionFailed", "ifNoneMatch: * refuses an overwrite");
  }
  try {
    await s3.file("cond.txt", { ifNoneMatch: first.etag }).text();
    ok(false, "ifNoneMatch on the current ETag");
  } catch (error) {
    is(error.status, 304, "ifNoneMatch on the current ETag is 304");
  }

  // -- copy -----------------------------------------------------------------
  await s3.copy("hello.txt", "copied/hello.txt");
  is(await s3.file("copied/hello.txt").text(), "hello from esrun", "copy");
  await s3.copy("meta.txt", "copied/meta.txt", { metadata: { owner: "copied" } });
  is((await s3.stat("copied/meta.txt")).metadata.owner, "copied", "copy with new metadata");

  // -- multipart copy -------------------------------------------------------
  // A real server's UploadPartCopy, driven directly: no server here refuses a
  // 12 MiB single copy, so copy()'s fallback would never reach it.
  {
    const source = await headSource(core, bucket, "big.bin", undefined, { retry: 2 });
    const copied = await multipartCopy(
      core,
      bucket,
      "copied/big.bin",
      bucket,
      "big.bin",
      source,
      { partSize: 5 * MiB },
      false,
    );
    ok(copied.etag.length > 2, "a multipart copy completes");
    const back = await s3.file("copied/big.bin").bytes();
    ok(
      back.length === big.length && back.every((b, i) => b === big[i]),
      "and the copy is byte for byte the source",
    );
  }

  // -- presign --------------------------------------------------------------
  const getUrl = await s3.presign("hello.txt", { expiresIn: 300 });
  is(
    await (await fetch(getUrl)).text(),
    "hello from esrun",
    "a presigned GET works with no credentials",
  );
  const putUrl = await s3.presign("via-link.txt", { method: "PUT", expiresIn: 300 });
  const put = await fetch(putUrl, { method: "PUT", body: "uploaded by link" });
  ok(put.ok, `a presigned PUT is accepted (${put.status})`);
  is(await s3.file("via-link.txt").text(), "uploaded by link", "and lands");

  // -- signing is the server's check, not ours ------------------------------
  const wrong = new S3Client({
    endpoint,
    region,
    bucket,
    accessKeyId,
    secretAccessKey: `${secretAccessKey}x`,
    retry: 0,
  });
  try {
    await wrong.file("hello.txt").text();
    ok(false, "a wrong secret is refused");
  } catch (error) {
    is(error.status, 403, "a wrong secret is refused with 403");
  }
} finally {
  // Empty the bucket and remove it.
  const all = [];
  for await (const item of s3.listAll()) all.push(item.key);
  if (all.length > 0) {
    const { errors } = await s3.deleteMany(all);
    is(errors.length, 0, "deleteMany empties the bucket");
  }
  let left = 0;
  for await (const _ of s3.listAll()) left++;
  is(left, 0, "nothing is left");
  await core.send({ method: "DELETE", bucket });
}

if (report("e2e") > 0) exit(1);
