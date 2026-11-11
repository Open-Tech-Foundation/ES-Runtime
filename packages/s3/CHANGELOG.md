# Changelog for `@opentf/esrun-s3`

All notable changes to **`@opentf/esrun-s3`**, the S3 client for ES Runtime,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `fetch`. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

### Added

- The `@opentf/esrun-s3` package (DECISIONS D211): an S3 client in JavaScript
  over `fetch`, signed with SigV4, for AWS S3, Cloudflare R2, Google Cloud
  Storage and S3-compatible stores. Bun's `S3Client`/`S3File` surface —
  `file()`, `text()`, `json()`, `bytes()`, `stream()`, `slice()` ranges,
  `write()`, `writer()`, `exists()`, `stat()`, `delete()`, `presign()`,
  `list()` — plus `listAll()`, `copy()` and `deleteMany()`. Multipart uploads
  for large blobs and streams with bounded memory, aborted on failure;
  conditional reads and writes; CRC32, CRC32C, SHA-1 and SHA-256 checksums;
  SSE-S3, SSE-KMS and SSE-C (HTTPS only); retries with backoff and clock-skew
  correction; refreshing credential providers; `S3Client.fromEnv()` reading
  Bun's `S3_*` names and the AWS ones. Objects are read byte for byte,
  `Content-Encoding` included (`decompress: false`, D210), so it needs esrun
  0.47.0 or later.
