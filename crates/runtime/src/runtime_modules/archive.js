// runtime:archive — tar, tar.gz and zip in and out (DECISIONS D174).
//
//   import { Archive } from "runtime:archive";
//
//   const archive = new Archive({
//     "hello.txt": "Hello, world!",
//     "assets/logo.png": png,
//   });
//   const bytes = await archive.bytes();        // a tar by default
//   await archive.extract("./out");             // needs FileWrite
//
//   const opened = new Archive(file("./bundle.tar.gz"));
//   const files = await opened.files();         // Map<string, File>
//
// Bytes in and out need no capability. Reading a file() source is FileRead
// (the same gate as reading that file), extraction is FileWrite, and building
// from bytes is pure. Nothing trusts a filename: the format is read from the
// leading bytes, and every entry path is validated by the host before an
// archive is listed or a file is written.
//
// A file() source is not read into memory here. The host opens and scans it,
// keeps the metadata, and reads only the bytes a caller asks for — in chunks
// for stream() and entry by entry for files()/extract().

const ops = globalThis.__ops;
const FILE_BODY = globalThis.__internal.fileBody;
const BLOB_BYTES = globalThis.__internal.bytes;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

// Module-private seams: entries hold an Archive and ask it for the one open
// handle, so no method has to carry the source bytes or limits around.
const OPEN = Symbol("archive open");

const MIME = {
  tar: "application/x-tar",
  "tar.gz": "application/gzip",
  zip: "application/zip",
};

const archiveHandles = new FinalizationRegistry((handle) => {
  ops.archive_close(handle);
});
const streamHandles = new FinalizationRegistry((id) => {
  ops.archive_stream_close(id);
});
const metaHandles = new FinalizationRegistry((id) => {
  ops.archive_entries_close(id);
});

function isFile(value) {
  return value !== null && typeof value === "object" && typeof value[FILE_BODY] === "function";
}

function isPlainObject(value) {
  if (value === null || typeof value !== "object") return false;
  const proto = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

function isShared(buffer) {
  return typeof SharedArrayBuffer === "function" && buffer instanceof SharedArrayBuffer;
}

// Copied, as a Blob copies its parts: an Archive is built or opened lazily, so
// a view would let a later write to the caller's buffer change what it holds.
function bytesOf(value, what) {
  let buffer;
  if (value instanceof ArrayBuffer) buffer = value;
  else if (ArrayBuffer.isView(value)) buffer = value.buffer;
  else throw new TypeError(`${what} must be a Uint8Array, an ArrayBuffer or a view`);
  if (isShared(buffer)) {
    throw new TypeError(`${what} cannot be a SharedArrayBuffer, which may change while it is read`);
  }
  if (value instanceof ArrayBuffer) return new Uint8Array(value.slice(0));
  return new Uint8Array(value.buffer, value.byteOffset, value.byteLength).slice();
}

function optionsObject(options, what) {
  if (options === undefined || options === null) return {};
  if (typeof options !== "object") throw new TypeError(`${what} options must be an object`);
  return options;
}

function limitOf(options, name) {
  const value = options[name];
  if (value === undefined) return undefined;
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new TypeError(`the archive ${name} limit must be a non-negative number`);
  }
  return Math.floor(value);
}

// `{ maxEntries, maxBytes }`, with only the limits the caller set, so the host
// defaults apply to the rest.
function limitsOf(options) {
  const limits = {};
  const entries = limitOf(options, "maxEntries");
  const bytes = limitOf(options, "maxBytes");
  if (entries !== undefined) limits.maxEntries = entries;
  if (bytes !== undefined) limits.maxBytes = bytes;
  return Object.keys(limits).length === 0 ? undefined : limits;
}

function formatOf(options) {
  const format = options.format ?? "tar";
  if (format !== "tar" && format !== "zip") {
    throw new TypeError(`unknown archive format ${JSON.stringify(format)}; expected "tar" or "zip"`);
  }
  return format;
}

function compressionOf(format, options) {
  const compress = options.compress ?? "none";
  if (format === "tar") {
    if (compress === "none") return "none";
    if (compress === "gzip") return "gzip";
    throw new TypeError(
      `a tar archive cannot use compression ${JSON.stringify(compress)}; expected "none" or "gzip"`,
    );
  }
  if (compress === "none") return "none";
  if (compress === "deflate") return "deflate";
  throw new TypeError(
    `a zip archive cannot use compression ${JSON.stringify(compress)}; expected "none" or "deflate"`,
  );
}

function lastModifiedOf(value, what) {
  if (value === undefined || value === null) return null;
  if (value instanceof Date) {
    const ms = value.getTime();
    if (!Number.isFinite(ms) || ms < 0) throw new TypeError(`${what} is not a valid date`);
    return Math.floor(ms);
  }
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new TypeError(`${what} must be a non-negative number of milliseconds or a Date`);
  }
  return Math.floor(value);
}

// One value in a build input, as entry bytes (or `null` for a directory).
function entryData(value, path) {
  if (value === null) {
    if (!path.endsWith("/")) {
      throw new TypeError(`entry ${JSON.stringify(path)}: a null value describes a directory, whose path must end with /`);
    }
    return { data: null, lastModified: null };
  }
  if (value instanceof Blob) {
    return {
      data: value[BLOB_BYTES](),
      lastModified: value instanceof File ? value.lastModified : null,
    };
  }
  if (typeof value === "string") return { data: encoder.encode(value), lastModified: null };
  if (typeof value === "object" && (value instanceof ArrayBuffer || ArrayBuffer.isView(value))) {
    return { data: bytesOf(value, `entry ${JSON.stringify(path)}`), lastModified: null };
  }
  if (isPlainObject(value) && "data" in value) {
    const data = value.data;
    if (data === null) {
      if (!path.endsWith("/")) {
        throw new TypeError(`entry ${JSON.stringify(path)}: a directory path must end with /`);
      }
      return { data: null, lastModified: lastModifiedOf(value.lastModified, `entry ${JSON.stringify(path)} lastModified`) };
    }
    let bytes;
    if (typeof data === "string") bytes = encoder.encode(data);
    else if (data instanceof Blob) bytes = data[BLOB_BYTES]();
    else bytes = bytesOf(data, `entry ${JSON.stringify(path)} data`);
    return {
      data: bytes,
      lastModified: lastModifiedOf(value.lastModified, `entry ${JSON.stringify(path)} lastModified`),
    };
  }
  throw new TypeError(
    `entry ${JSON.stringify(path)} must be a string, bytes, a Blob or File, or { data, lastModified }`,
  );
}

function buildEntries(input) {
  const entries = [];
  for (const [path, value] of Object.entries(input)) {
    const { data, lastModified } = entryData(value, path);
    entries.push({ path, data, lastModified });
  }
  return entries;
}

function readOptions(options) {
  return limitsOf(options);
}

function matches(filter, path) {
  if (filter === undefined || filter === null) return true;
  if (typeof filter === "function") return !!filter(path);
  if (typeof filter === "string") return path.startsWith(filter);
  if (filter instanceof RegExp) {
    filter.lastIndex = 0;
    return filter.test(path);
  }
  throw new TypeError("a files() filter must be a function, a string prefix or a RegExp");
}

class ArchiveEntry {
  #archive;
  #meta;

  constructor(archive, meta) {
    this.#archive = archive;
    this.#meta = meta;
  }

  get path() {
    return this.#meta.path;
  }

  get size() {
    return this.#meta.size;
  }

  get directory() {
    return this.#meta.directory;
  }

  get lastModified() {
    return this.#meta.lastModified;
  }

  async bytes() {
    if (this.directory) return new Uint8Array(0);
    const { handle } = await this.#archive[OPEN]();
    const entries = await ops.archive_read_selected(handle, [this.path]);
    return entries[0].data;
  }

  async text() {
    return decoder.decode(await this.bytes());
  }

  async blob() {
    return new Blob([await this.bytes()]);
  }

  stream() {
    if (this.directory) return new ReadableStream({ start: (controller) => controller.close() });
    const archive = this.#archive;
    const path = this.path;
    const token = {};
    let id = null;
    let cancelled = false;
    let finished = false;

    return new ReadableStream({
      async pull(controller) {
        if (finished || cancelled) return;
        try {
          if (id === null) {
            const { handle } = await archive[OPEN]();
            if (cancelled) return;
            id = ops.archive_stream_open(handle, path);
            streamHandles.register(token, id, token);
          }
          const { data, done } = await ops.archive_stream_next(id);
          if (cancelled) return;
          if (data.byteLength) controller.enqueue(data);
          if (done) {
            finished = true;
            streamHandles.unregister(token);
            id = null;
            controller.close();
          }
        } catch (error) {
          if (cancelled) return;
          finished = true;
          streamHandles.unregister(token);
          id = null;
          controller.error(error);
        }
      },
      cancel() {
        cancelled = true;
        if (id !== null) {
          streamHandles.unregister(token);
          ops.archive_stream_close(id);
          id = null;
        }
      },
    });
  }
}

class Archive {
  #source;
  #limits;
  #format = null;
  #compress = "none";
  #built = null;
  #opening = null;

  constructor(input, options = {}) {
    const opts = optionsObject(options, "Archive");
    if (isFile(input)) {
      this.#source = { path: input[FILE_BODY]() };
      this.#limits = readOptions(opts);
      return;
    }
    if (input instanceof Blob) {
      this.#source = { bytes: input[BLOB_BYTES]() };
      this.#limits = readOptions(opts);
      return;
    }
    if (typeof input === "object" && input !== null && (input instanceof ArrayBuffer || ArrayBuffer.isView(input))) {
      this.#source = { bytes: bytesOf(input, "an Archive source") };
      this.#limits = readOptions(opts);
      return;
    }
    if (isPlainObject(input)) {
      this.#format = formatOf(opts);
      this.#compress = compressionOf(this.#format, opts);
      this.#limits = readOptions(opts);
      this.#source = { entries: buildEntries(input) };
      return;
    }
    throw new TypeError(
      "an Archive takes a plain object of entries, bytes, a Blob or a file(); pass file(path) from runtime:fs to read a file",
    );
  }

  // One open per Archive: the host keeps the source and format, so entry reads,
  // listings and extraction share the same file handle. Metadata is not
  // collected here; entries() asks for a snapshot and the async iterator
  // streams it.
  [OPEN]() {
    if (this.#opening !== null) return this.#opening;
    this.#opening = (async () => {
      let opened;
      if (this.#source.entries !== undefined) {
        opened = await ops.archive_open_bytes(this.#builtBytes(), this.#limits);
      } else if (this.#source.path !== undefined) {
        opened = await ops.archive_open_file(this.#source.path, this.#limits);
      } else {
        opened = await ops.archive_open_bytes(this.#source.bytes, this.#limits);
      }
      this.#format = opened.format;
      archiveHandles.register(this, opened.handle, this);
      return opened;
    })().catch((error) => {
      this.#opening = null;
      throw error;
    });
    return this.#opening;
  }

  #builtBytes() {
    if (this.#built === null) {
      this.#built = ops.archive_build(
        this.#source.entries,
        this.#format,
        this.#compress,
        this.#limits,
      );
    }
    return this.#built;
  }

  // A copy, always: mutating the result never changes what this Archive will
  // produce next time, and an opened archive's source bytes are left alone.
  async bytes() {
    if (this.#source.entries !== undefined) return this.#builtBytes().slice();
    if (this.#source.bytes !== undefined) return this.#source.bytes.slice();
    const { handle } = await this[OPEN]();
    return ops.archive_source_bytes(handle);
  }

  async blob() {
    const { format } = await this[OPEN]();
    return new Blob([await this.bytes()], { type: MIME[format] });
  }

  // Metadata as an async iterator, parsed and validated one entry at a time.
  // `entries()` is the eager snapshot; this keeps only the current entry in the
  // guest's heap, though the host retains the paths it needs for collision
  // checks and a zip parser retains its central directory.
  async *[Symbol.asyncIterator]() {
    const { handle } = await this[OPEN]();
    const token = {};
    let id = ops.archive_entries_open(handle);
    metaHandles.register(token, id, token);
    try {
      for (;;) {
        const { entry, done } = await ops.archive_entries_next(id);
        if (done) return;
        yield new ArchiveEntry(this, entry);
      }
    } finally {
      metaHandles.unregister(token);
      ops.archive_entries_close(id);
    }
  }

  async entries() {
    const { handle } = await this[OPEN]();
    const entries = await ops.archive_entries(handle);
    return entries.map((meta) => new ArchiveEntry(this, meta));
  }

  async files(filter) {
    const { handle } = await this[OPEN]();
    const selected = [];
    for await (const entry of this) {
      if (!entry.directory && matches(filter, entry.path)) selected.push(entry);
    }
    const files = new Map();
    if (selected.length === 0) return files;
    const data = await ops.archive_read_selected(handle, selected.map((entry) => entry.path));
    const byPath = new Map(data.map((entry) => [entry.path, entry.data]));
    for (const entry of selected) {
      const bytes = byPath.get(entry.path);
      if (bytes === undefined) continue;
      const options = entry.lastModified === null ? {} : { lastModified: entry.lastModified };
      files.set(entry.path, new File([bytes], entry.path, options));
    }
    return files;
  }

  // Writes the archive's contents under `directory`, which must be a path
  // string or a file() handle. Needs FileWrite; the host validates every entry
  // before it writes the first one.
  async extract(directory, options = {}) {
    const path = isFile(directory) ? directory[FILE_BODY]() : directory;
    if (typeof path !== "string") {
      throw new TypeError("extract() takes a directory path or a file()");
    }
    const opts = optionsObject(options, "extract");
    // A limit passed here overrides that one limit; the others stay the ones
    // the Archive was opened with.
    const limits = { ...this.#limits, ...limitsOf(opts) };
    const { handle } = await this[OPEN]();
    return ops.archive_extract(handle, path, limits);
  }

  // Writes an archive built from `input` to `destination` (a path string or a
  // file()). Needs FileWrite and resolves to the number of bytes written.
  static async write(destination, input, options = {}) {
    const path = isFile(destination) ? destination[FILE_BODY]() : destination;
    if (typeof path !== "string") {
      throw new TypeError("Archive.write() takes a destination path or a file()");
    }
    const archive = input instanceof Archive ? input : new Archive(input, options);
    return ops.fs_write(path, await archive.bytes(), false);
  }
}

export { Archive, ArchiveEntry };
