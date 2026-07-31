// runtime:fs — modern, Blob-based file I/O (SPEC §11, DECISIONS D25). An ES
// module backed by async ops: reads gated on FileRead, mutations on FileWrite,
// every path confined to the provider's root jail. Modeled on the web Blob
// surface (the Bun.file shape) rather than the legacy Node fs API: file() is a
// lazy, Blob-like handle; write() takes any web body; whole-file + stream reads,
// no sync variants, no callbacks. Stream-to-disk is write(dest, stream).

const ops = globalThis.__ops;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

// Accepts a path string, a file: URL (string or URL), or a file() handle. file:
// URLs are handled inline so this module needs no Env (no runtime:process).
function pathOf(p) {
  if (p instanceof FsFile) return p.path;
  if (p instanceof URL) return urlToPath(p);
  if (typeof p === "string") return p.startsWith("file://") ? urlToPath(p) : p;
  throw new TypeError("path must be a string, URL, or file() handle");
}

function urlToPath(u) {
  const url = u instanceof URL ? u : new URL(u);
  if (url.protocol !== "file:") throw new TypeError(`expected a file: URL, got ${url.protocol}`);
  let p = decodeURIComponent(url.pathname);
  if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1); // Windows drive: /C:/x -> C:/x
  return p;
}

// A lazy, Blob-like file handle. Nothing is read until a read method is called.
class FsFile {
  constructor(path) {
    this.path = path;
  }
  async bytes() {
    return ops.fs_read(this.path); // Uint8Array
  }
  async arrayBuffer() {
    const b = await this.bytes();
    return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
  }
  async text() {
    return decoder.decode(await this.bytes());
  }
  async json() {
    return JSON.parse(await this.text());
  }
  async exists() {
    return ops.fs_exists(this.path);
  }
  async stat() {
    return await ops.fs_stat(this.path);
  }
  async write(data, options) {
    return write(this.path, data, options);
  }
  async delete() {
    return ops.fs_remove(this.path, false);
  }
  // A web-standard WritableStream sink for incremental / piped writes:
  //   await readable.pipeTo(file("out.log").writable({ append: true }));
  // The first chunk truncates the file unless append is true; later chunks always append.
  writable(options = {}) {
    const path = this.path;
    let started = !!options.append;
    return new WritableStream({
      async write(chunk) {
        await ops.fs_write(path, chunkToBytes(chunk), started);
        started = true;
      },
    });
  }
  stream() {
    const path = this.path;
    return new ReadableStream({
      async pull(controller) {
        const bytes = await ops.fs_read(path);
        if (bytes.byteLength) controller.enqueue(bytes);
        controller.close();
      },
    });
  }
}

function chunkToBytes(chunk) {
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (chunk instanceof Uint8Array) return chunk;
  if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  throw new TypeError("FileSink.write expects a string, ArrayBuffer, or ArrayBufferView");
}

function concat(chunks) {
  let total = 0;
  for (const c of chunks) total += c.byteLength;
  const out = new Uint8Array(total);
  let offset = 0;
  for (const c of chunks) {
    out.set(c, offset);
    offset += c.byteLength;
  }
  return out;
}

function file(path) {
  return new FsFile(pathOf(path));
}

// Glob matching and scanning. `match(path)` is pure pattern matching (sync, no
// capability); `scan()` walks the jailed filesystem (needs FileRead) and is an
// async iterator. Patterns support *, **, ?, [classes], and {a,b} alternation.
class Glob {
  constructor(pattern) {
    this.pattern = pattern;
  }
  match(path) {
    return ops.glob_match(this.pattern, pathOf(path));
  }
  async *scan(options = ".") {
    const opts = typeof options === "string" ? { cwd: options } : options || {};
    const cwd = opts.cwd ?? ".";
    const json = await ops.glob_scan(
      pathOf(cwd),
      this.pattern,
      !!opts.dot,
      !!opts.absolute,
      opts.onlyFiles !== false, // default: files only, like the prior art
      !!opts.followSymlinks,
    );
    for (const p of json) yield p;
  }
}

async function toBytes(input) {
  if (typeof input === "string") return encoder.encode(input);
  if (input instanceof Uint8Array) return input;
  if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  if (input instanceof ArrayBuffer) return new Uint8Array(input);
  if (input instanceof FsFile) return input.bytes();
  if (input instanceof Blob) return new Uint8Array(await input.arrayBuffer());
  if (input instanceof Response) return new Uint8Array(await input.arrayBuffer());
  if (input instanceof ReadableStream) return drain(input);
  throw new TypeError("write input must be a string, Blob, ArrayBuffer, TypedArray, Response, ReadableStream, or file()");
}

async function drain(stream) {
  const reader = stream.getReader();
  const chunks = [];
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value instanceof Uint8Array ? value : new Uint8Array(value));
  }
  return concat(chunks);
}

// Writes any web body to `dest`; resolves to the number of bytes written. Pass
// { append: true } to add to the end instead of truncating.
//
// A string is passed through untouched: the op encodes it to UTF-8 on the Rust
// side in one pass, avoiding a `TextEncoder` round-trip and an extra buffer copy
// across the boundary. Everything else is normalized to bytes first.
async function write(dest, input, options = {}) {
  const payload = typeof input === "string" ? input : await toBytes(input);
  return ops.fs_write(pathOf(dest), payload, !!options.append);
}

async function readDir(path) {
  return await ops.fs_read_dir(pathOf(path));
}

async function stat(path) {
  return await ops.fs_stat(pathOf(path));
}

async function exists(path) {
  return ops.fs_exists(pathOf(path));
}

async function mkdir(path, { recursive = false } = {}) {
  return ops.fs_mkdir(pathOf(path), !!recursive);
}

async function remove(path, { recursive = false } = {}) {
  return ops.fs_remove(pathOf(path), !!recursive);
}

async function rename(from, to) {
  return ops.fs_rename(pathOf(from), pathOf(to));
}

// Copies a file, overwriting the destination; resolves to bytes copied. The op
// needs both FileRead and FileWrite — it reads one path and writes another, and
// gating it on the write alone would let a guest duplicate a file it cannot
// read into somewhere it can reach another way.
async function copy(from, to) {
  return ops.fs_copy(pathOf(from), pathOf(to));
}

// The canonical location of `path`: symlinks followed, `.`/`..` removed. Throws
// if it does not exist, or resolves outside the root jail.
async function realPath(path) {
  return ops.fs_real_path(pathOf(path));
}

// The stored target of a symbolic link, verbatim — possibly relative, possibly
// dangling. `realPath` is what resolves it.
async function readLink(path) {
  return ops.fs_read_link(pathOf(path));
}

// Sets the file's length exactly, zero-filling if it grows.
async function truncate(path, len = 0) {
  return ops.fs_truncate(pathOf(path), Number(len) || 0);
}

// Sets permission bits (a Unix mode such as 0o600). On Windows only the
// owner-write bit is representable, as the read-only flag.
async function chmod(path, mode) {
  if (typeof mode !== "number" || !Number.isInteger(mode) || mode < 0) {
    throw new TypeError("chmod: mode must be a non-negative integer (e.g. 0o600)");
  }
  return ops.fs_chmod(pathOf(path), mode);
}

// Creates a directory (or file) with an unpredictable name and returns its
// path. `dir` defaults to the base directory rather than the OS temp directory:
// that lives outside the root jail, so writing there would be the one
// filesystem call that escapes it. The caller owns what it gets — nothing is
// cleaned up automatically.
async function makeTempDir({ dir = "", prefix = "" } = {}) {
  return ops.fs_make_temp_dir(dir ? pathOf(dir) : "", String(prefix));
}

async function makeTempFile({ dir = "", prefix = "" } = {}) {
  return ops.fs_make_temp_file(dir ? pathOf(dir) : "", String(prefix));
}

export {
  file,
  write,
  readDir,
  stat,
  exists,
  mkdir,
  remove,
  rename,
  copy,
  realPath,
  readLink,
  truncate,
  chmod,
  makeTempDir,
  makeTempFile,
  Glob,
};
export default {
  file,
  write,
  readDir,
  stat,
  exists,
  mkdir,
  remove,
  rename,
  copy,
  realPath,
  readLink,
  truncate,
  chmod,
  makeTempDir,
  makeTempFile,
  Glob,
};
