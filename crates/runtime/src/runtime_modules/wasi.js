// `runtime:wasi` — WASI preview 1 (`wasi_snapshot_preview1`).
//
// Enough of the ABI to run what the `wasm32-wasip1` toolchains emit: arguments,
// environment, clocks, randomness, stdio, process exit, and — when the embedder
// grants a directory — the filesystem.
//
// ## No ambient authority (D5)
//
// Unlike Node's `node:wasi`, arguments and environment come **only** from what
// the caller passes to the constructor. There is no path by which a guest module
// reads the host's real environment through this module, so no capability is
// needed to construct one and none is silently inherited. An embedder that wants
// to forward the real environment does so explicitly, having already gone
// through the `Env`-gated `runtime:process`.
//
// ## Filesystem
//
// A guest sees only what `preopens` maps in — WASI's own model, and the reason
// its file calls are all relative to a directory fd. Reaching a file therefore
// passes *three* checks: the preopen must map it, the host op must hold the
// `FileRead`/`FileWrite` capability, and the provider's root jail must contain
// the resolved path (DECISIONS D25). With no preopens (or no synchronous
// filesystem installed) every file call reports `ENOTCAPABLE`.
//
// The syscalls are synchronous — a guest calls `fd_read` and expects bytes back
// with no chance to yield — so they go through the blocking `sync_fs_*` ops
// rather than the async `runtime:fs` ones.
//
// Every preview-1 import is defined regardless of what is available: a missing
// import is a `LinkError` at instantiation, which would fail a program that
// merely links a symbol without ever calling it.

const ERRNO = {
  SUCCESS: 0,
  BADF: 8,
  EXIST: 20,
  FAULT: 21,
  INVAL: 28,
  IO: 29,
  ISDIR: 31,
  NOENT: 44,
  NOSYS: 52,
  NOTDIR: 54,
  NOTEMPTY: 55,
  NOTCAPABLE: 76,
  PERM: 63,
  SPIPE: 70,
};

/// WASI clock ids.
const CLOCK_REALTIME = 0;
const CLOCK_MONOTONIC = 1;

const FILETYPE_UNKNOWN = 0;
const FILETYPE_DIRECTORY = 3;
const FILETYPE_REGULAR_FILE = 4;
const FILETYPE_CHARACTER_DEVICE = 2;

/// `fd_fdstat_get` writes 24 bytes; stdio is a character device.
const FDSTAT_SIZE = 24;

/// `filestat` is 64 bytes: dev(8) ino(8) filetype(1)+pad(7) nlink(8) size(8)
/// atim(8) mtim(8) ctim(8).
const FILESTAT_SIZE = 64;

/// `path_open` oflags.
const OFLAGS_CREAT = 1 << 0;
const OFLAGS_DIRECTORY = 1 << 1;
const OFLAGS_EXCL = 1 << 2;
const OFLAGS_TRUNC = 1 << 3;

/// `fdflags`.
const FDFLAGS_APPEND = 1 << 0;

/// The rights a write needs. WASI encodes rights as a 64-bit mask; these are the
/// two bits that decide whether an open is a writing one.
const RIGHT_FD_WRITE = 1n << 6n;
const RIGHT_FD_ALLOCATE = 1n << 8n;

/// The first fd handed to a preopen. 0/1/2 are stdio, so preopens start at 3 —
/// which is the convention every WASI program's libc assumes.
const FIRST_PREOPEN_FD = 3;

/// Maps a host error to the closest WASI errno. The host reports a message and
/// an error code, not an errno, so this is a best-effort classification; the
/// fallback is `EIO`, which callers treat as a generic failure.
function errnoFor(e) {
  const text = `${(e && e.message) || e}`.toLowerCase();
  if (e && e.code === "ERR_JAIL_ESCAPE") return ERRNO.NOTCAPABLE;
  if (e && e.code === "ERR_NOT_FOUND") return ERRNO.NOENT;
  if (text.includes("not found") || text.includes("no such file")) return ERRNO.NOENT;
  if (text.includes("already exists")) return ERRNO.EXIST;
  if (text.includes("not a directory")) return ERRNO.NOTDIR;
  if (text.includes("is a directory")) return ERRNO.ISDIR;
  if (text.includes("not empty")) return ERRNO.NOTEMPTY;
  if (text.includes("permission") || text.includes("capability")) return ERRNO.NOTCAPABLE;
  return ERRNO.IO;
}

function filetypeOf(stat) {
  if (stat.isDir) return FILETYPE_DIRECTORY;
  if (stat.isFile) return FILETYPE_REGULAR_FILE;
  return FILETYPE_UNKNOWN;
}

/// Thrown by `proc_exit` and unwound to `start`/`initialize`, which report the
/// code. A WASI `_start` returning normally is exit status 0, and calling
/// `proc_exit` is the other normal way to finish — neither is an error here.
class ExitStatus extends Error {
  constructor(code) {
    super(`WASI program exited with status ${code}`);
    this.name = "ExitStatus";
    this.code = code;
  }
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/// Line-buffers bytes written to a stdio fd and forwards complete lines to the
/// console. WASI writes raw bytes with its own newlines, while the console sink
/// is line-oriented, so buffering is what keeps a `print("a"); print("b\n")` from
/// becoming two lines.
class LineWriter {
  #emit;
  #buffered = "";

  constructor(emit) {
    this.#emit = emit;
  }

  write(bytes) {
    // `stream: true` so a multi-byte character split across two writes is not
    // mangled into replacement characters.
    this.#buffered += decoder.decode(bytes, { stream: true });
    let newline;
    while ((newline = this.#buffered.indexOf("\n")) !== -1) {
      this.#emit(this.#buffered.slice(0, newline));
      this.#buffered = this.#buffered.slice(newline + 1);
    }
  }

  /// Emits anything after the last newline. Called when the program finishes so
  /// a final unterminated write is not swallowed.
  flush() {
    if (this.#buffered !== "") {
      this.#emit(this.#buffered);
      this.#buffered = "";
    }
  }
}

/// A WASI preview 1 instance: build one, hand `getImportObject()` to
/// `WebAssembly.instantiate`, then `start(instance)`.
export class WASI {
  #args;
  #env;
  #memory = null;
  #stdout;
  #stderr;
  /// fd → descriptor. Populated with the preopens at construction; `path_open`
  /// adds to it. Stdio (0/1/2) is handled separately and never appears here.
  #fds = new Map();
  #nextFd = FIRST_PREOPEN_FD;

  /**
   * @param {object} [options]
   * @param {string[]} [options.args] `argv`, including `argv[0]`.
   * @param {Record<string,string>} [options.env] Environment pairs.
   * @param {Record<string,string>} [options.preopens] Guest directory → host
   *   directory. The guest sees only these; everything else is `ENOTCAPABLE`.
   * @param {string} [options.version] Must be `"preview1"` if given.
   */
  constructor(options = {}) {
    const {
      args = [],
      env = {},
      preopens = {},
      version = "preview1",
    } = options ?? {};
    if (version !== "preview1") {
      throw new TypeError(
        `unsupported WASI version ${JSON.stringify(version)} (only "preview1")`,
      );
    }
    if (!Array.isArray(args)) throw new TypeError("args must be an array");
    this.#args = args.map(String);
    this.#env = Object.entries(env).map(([k, v]) => `${k}=${v}`);
    this.#stdout = new LineWriter((line) => console.log(line));
    this.#stderr = new LineWriter((line) => console.error(line));

    // Preopens occupy the lowest fds in insertion order, which is what a guest's
    // libc walks at startup via fd_prestat_get until it gets EBADF.
    for (const [guestPath, hostPath] of Object.entries(preopens)) {
      this.#fds.set(this.#nextFd++, {
        kind: "dir",
        preopen: guestPath,
        host: String(hostPath),
        // A preopened directory has no host handle: it is only ever an anchor
        // for resolving paths, so nothing needs opening until a file is.
        handle: null,
      });
    }
  }

  /// Resolves a guest path against the directory `fd`, returning the host path.
  ///
  /// Rejects anything that would climb out of the directory it is anchored to.
  /// The provider's root jail is the real boundary — this only ensures a guest
  /// cannot address outside its own preopen, so two preopens stay separate.
  #resolve(fd, path) {
    const entry = this.#fds.get(fd);
    if (!entry || entry.kind !== "dir") return null;

    const segments = [];
    for (const segment of String(path).split("/")) {
      if (segment === "" || segment === ".") continue;
      if (segment === "..") {
        // Refuse to climb above the anchor rather than silently clamping, so a
        // traversal attempt is an error the guest sees.
        if (segments.length === 0) return null;
        segments.pop();
        continue;
      }
      segments.push(segment);
    }
    return segments.length === 0 ? entry.host : `${entry.host}/${segments.join("/")}`;
  }

  /// Reads a guest string from `(ptr, len)`.
  #string(ptr, len) {
    return decoder.decode(this.#bytes().subarray(ptr, ptr + len));
  }

  /// The `wasi_snapshot_preview1` import object.
  getImportObject() {
    return { wasi_snapshot_preview1: this.#exports() };
  }

  /// Runs a command module: binds its memory, calls `_start`, and returns the
  /// exit status (0 when `_start` returns without calling `proc_exit`).
  start(instance) {
    this.#bind(instance);
    const start = instance.exports._start;
    if (typeof start !== "function") {
      throw new TypeError("WASI command module does not export _start");
    }
    return this.#run(start);
  }

  /// Runs a reactor module: binds its memory and calls `_initialize` if present,
  /// leaving the instance live for the caller to drive.
  initialize(instance) {
    this.#bind(instance);
    const init = instance.exports._initialize;
    if (init !== undefined && typeof init !== "function") {
      throw new TypeError("_initialize is not a function");
    }
    if (init) this.#run(init);
  }

  #run(fn) {
    try {
      fn();
      return 0;
    } catch (e) {
      if (e instanceof ExitStatus) return e.code;
      throw e;
    } finally {
      // Whatever happened, do not strand buffered output.
      this.#stdout.flush();
      this.#stderr.flush();
    }
  }

  #bind(instance) {
    const memory = instance?.exports?.memory;
    if (!(memory instanceof WebAssembly.Memory)) {
      throw new TypeError("WASI instance does not export its memory");
    }
    this.#memory = memory;
  }

  /// A `DataView` over current memory. Re-read every time: `memory.grow()`
  /// detaches the old `ArrayBuffer`, so a cached view would throw.
  #view() {
    if (this.#memory === null) {
      throw new TypeError("WASI instance memory is not bound yet");
    }
    return new DataView(this.#memory.buffer);
  }

  #bytes() {
    return new Uint8Array(this.#memory.buffer);
  }

  /// Reads a WASI `iovec` array — `(ptr, len)` pairs — into one byte array.
  #gather(iovsPtr, iovsLen) {
    const view = this.#view();
    const memory = this.#bytes();
    const chunks = [];
    let total = 0;
    for (let i = 0; i < iovsLen; i++) {
      const base = iovsPtr + i * 8;
      const ptr = view.getUint32(base, true);
      const len = view.getUint32(base + 4, true);
      chunks.push(memory.subarray(ptr, ptr + len));
      total += len;
    }
    const out = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) {
      out.set(chunk, offset);
      offset += chunk.length;
    }
    return out;
  }

  /// Writes a list of NUL-terminated strings in WASI's two-array layout: a
  /// pointer array and a packed buffer. Shared by `args_get`/`environ_get`.
  #writeStringList(list, pointersPtr, bufferPtr) {
    const view = this.#view();
    const memory = this.#bytes();
    let cursor = bufferPtr;
    for (let i = 0; i < list.length; i++) {
      view.setUint32(pointersPtr + i * 4, cursor, true);
      const encoded = encoder.encode(`${list[i]}\0`);
      memory.set(encoded, cursor);
      cursor += encoded.length;
    }
    return ERRNO.SUCCESS;
  }

  /// Runs a path-taking mutation resolved against a directory fd, mapping any
  /// host failure to an errno. Shared by mkdir/unlink/rmdir.
  #pathOp(dirFd, pathPtr, pathLen, run) {
    const host = this.#resolve(dirFd, this.#string(pathPtr, pathLen));
    if (host === null) return ERRNO.NOTCAPABLE;
    try {
      run(host);
      return ERRNO.SUCCESS;
    } catch (e) {
      return errnoFor(e);
    }
  }

  /// Writes WASI's 64-byte `filestat`. Only the fields a host stat can honestly
  /// fill are set; `dev`/`ino`/`nlink` stay zero rather than being invented.
  #writeFilestat(ptr, stat) {
    const view = this.#view();
    for (let i = 0; i < FILESTAT_SIZE; i++) view.setUint8(ptr + i, 0);
    view.setUint8(ptr + 16, filetypeOf(stat));
    view.setBigUint64(ptr + 24, BigInt(stat.size), true);
    // WASI timestamps are nanoseconds; the host reports milliseconds.
    const ns = BigInt(Math.round((stat.mtimeMs ?? 0) * 1e6));
    view.setBigUint64(ptr + 32, ns, true); // atim (best available)
    view.setBigUint64(ptr + 40, ns, true); // mtim
    view.setBigUint64(ptr + 48, ns, true); // ctim
    return ERRNO.SUCCESS;
  }

  /// Writes directory entries in WASI's packed `dirent` layout, resuming from
  /// `cookie`. Each record is a 24-byte header — next-cookie(8), inode(8),
  /// name length(4), filetype(1)+pad(3) — followed by the raw name.
  ///
  /// A record that does not fit is truncated rather than dropped: WASI signals
  /// "more to come" by filling the buffer exactly, and the guest retries with a
  /// larger one.
  #writeDirents(hostPath, bufPtr, bufLen, cookie, bufusedPtr) {
    const entries = __ops.sync_fs_readdir(hostPath);
    const memory = this.#bytes();
    const view = this.#view();
    let offset = 0;

    for (let i = Number(cookie); i < entries.length; i++) {
      const entry = entries[i];
      const name = encoder.encode(entry.name);
      const record = 24 + name.length;
      const room = bufLen - offset;
      if (room <= 0) break;

      const header = new DataView(new ArrayBuffer(24));
      header.setBigUint64(0, BigInt(i + 1), true); // d_next
      header.setBigUint64(8, 0n, true); // d_ino: not exposed by the host
      header.setUint32(16, name.length, true);
      header.setUint8(20, filetypeOf(entry));

      const bytes = new Uint8Array(record);
      bytes.set(new Uint8Array(header.buffer), 0);
      bytes.set(name, 24);
      memory.set(bytes.subarray(0, Math.min(record, room)), bufPtr + offset);
      offset += Math.min(record, room);
      if (record > room) break;
    }

    view.setUint32(bufusedPtr, offset, true);
    return ERRNO.SUCCESS;
  }

  #writeStringListSizes(list, countPtr, sizePtr) {
    const view = this.#view();
    view.setUint32(countPtr, list.length, true);
    const bytes = list.reduce((n, s) => n + encoder.encode(s).length + 1, 0);
    view.setUint32(sizePtr, bytes, true);
    return ERRNO.SUCCESS;
  }

  #exports() {
    // Every preview-1 import is defined, even the unimplemented ones: an absent
    // import is a LinkError at instantiation, which would break a program that
    // merely *links* the symbol without ever calling it.
    const notCapable = () => ERRNO.NOTCAPABLE;
    const noSys = () => ERRNO.NOSYS;

    return {
      // --- arguments and environment (constructor-provided only) ---
      args_get: (argvPtr, argvBufPtr) =>
        this.#writeStringList(this.#args, argvPtr, argvBufPtr),
      args_sizes_get: (argcPtr, argvBufSizePtr) =>
        this.#writeStringListSizes(this.#args, argcPtr, argvBufSizePtr),
      environ_get: (environPtr, environBufPtr) =>
        this.#writeStringList(this.#env, environPtr, environBufPtr),
      environ_sizes_get: (countPtr, bufSizePtr) =>
        this.#writeStringListSizes(this.#env, countPtr, bufSizePtr),

      // --- clocks ---
      clock_res_get: (id, resPtr) => {
        if (id !== CLOCK_REALTIME && id !== CLOCK_MONOTONIC) return ERRNO.INVAL;
        // `performance.now()` is milliseconds with microsecond precision.
        this.#view().setBigUint64(resPtr, 1000n, true);
        return ERRNO.SUCCESS;
      },
      clock_time_get: (id, _precision, timePtr) => {
        let ms;
        if (id === CLOCK_REALTIME) {
          ms = performance.timeOrigin + performance.now();
        } else if (id === CLOCK_MONOTONIC) {
          ms = performance.now();
        } else {
          return ERRNO.INVAL;
        }
        // WASI reports nanoseconds; go through BigInt so the value does not lose
        // precision past 2^53 as a Number would.
        const ns = BigInt(Math.round(ms * 1e6));
        this.#view().setBigUint64(timePtr, ns, true);
        return ERRNO.SUCCESS;
      },

      // --- randomness ---
      random_get: (bufPtr, bufLen) => {
        crypto.getRandomValues(this.#bytes().subarray(bufPtr, bufPtr + bufLen));
        return ERRNO.SUCCESS;
      },

      // --- process ---
      proc_exit: (code) => {
        throw new ExitStatus(code);
      },
      proc_raise: () => ERRNO.NOSYS,
      sched_yield: () => ERRNO.SUCCESS,

      // --- stdio ---
      fd_write: (fd, iovsPtr, iovsLen, nwrittenPtr) => {
        const bytes = this.#gather(iovsPtr, iovsLen);
        if (fd === 1) this.#stdout.write(bytes);
        else if (fd === 2) this.#stderr.write(bytes);
        else {
          const entry = this.#fds.get(fd);
          if (!entry || entry.kind !== "file") return ERRNO.BADF;
          try {
            const written = __ops.sync_fs_write(entry.handle, bytes);
            this.#view().setUint32(nwrittenPtr, written, true);
            return ERRNO.SUCCESS;
          } catch (e) {
            return errnoFor(e);
          }
        }
        this.#view().setUint32(nwrittenPtr, bytes.length, true);
        return ERRNO.SUCCESS;
      },
      fd_read: (fd, iovsPtr, iovsLen, nreadPtr) => {
        // No stdin source is wired: report a clean end-of-file rather than an
        // error, which is what a reader expects from an empty stream.
        if (fd === 0) {
          this.#view().setUint32(nreadPtr, 0, true);
          return ERRNO.SUCCESS;
        }
        const entry = this.#fds.get(fd);
        if (!entry || entry.kind !== "file") return ERRNO.BADF;
        try {
          // Fill each iovec in turn, stopping at end of file.
          const view = this.#view();
          let total = 0;
          for (let i = 0; i < iovsLen; i++) {
            const base = iovsPtr + i * 8;
            const ptr = view.getUint32(base, true);
            const len = view.getUint32(base + 4, true);
            if (len === 0) continue;
            const chunk = __ops.sync_fs_read(entry.handle, len);
            if (chunk.length === 0) break;
            this.#bytes().set(chunk, ptr);
            total += chunk.length;
            if (chunk.length < len) break; // short read: end of file
          }
          this.#view().setUint32(nreadPtr, total, true);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      fd_fdstat_get: (fd, statPtr) => {
        const view = this.#view();
        // Zero the struct first; only the filetype is worth advertising.
        for (let i = 0; i < FDSTAT_SIZE; i++) view.setUint8(statPtr + i, 0);
        if (fd <= 2) {
          view.setUint8(statPtr, FILETYPE_CHARACTER_DEVICE);
          return ERRNO.SUCCESS;
        }
        const entry = this.#fds.get(fd);
        if (!entry) return ERRNO.BADF;
        if (entry.kind === "dir") {
          view.setUint8(statPtr, FILETYPE_DIRECTORY);
          return ERRNO.SUCCESS;
        }
        try {
          view.setUint8(statPtr, filetypeOf(__ops.sync_fs_fstat(entry.handle)));
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      fd_close: (fd) => {
        if (fd <= 2) return ERRNO.SUCCESS;
        const entry = this.#fds.get(fd);
        if (!entry) return ERRNO.BADF;
        this.#fds.delete(fd);
        if (entry.handle === null) return ERRNO.SUCCESS;
        try {
          __ops.sync_fs_close(entry.handle);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      // stdio is a pipe: seeking it is invalid, which is what ESPIPE says.
      fd_seek: (fd, offset, whence, newOffsetPtr) => {
        if (fd <= 2) return ERRNO.SPIPE;
        const entry = this.#fds.get(fd);
        if (!entry || entry.kind !== "file") return ERRNO.BADF;
        try {
          const pos = __ops.sync_fs_seek(entry.handle, Number(offset), whence);
          this.#view().setBigUint64(newOffsetPtr, BigInt(pos), true);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      fd_tell: (fd, offsetPtr) => {
        if (fd <= 2) return ERRNO.SPIPE;
        const entry = this.#fds.get(fd);
        if (!entry || entry.kind !== "file") return ERRNO.BADF;
        try {
          // Whence 1 (current) with a zero offset reports the position.
          const pos = __ops.sync_fs_seek(entry.handle, 0, 1);
          this.#view().setBigUint64(offsetPtr, BigInt(pos), true);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      fd_fdstat_set_flags: () => ERRNO.SUCCESS,
      fd_sync: () => ERRNO.SUCCESS,
      fd_datasync: () => ERRNO.SUCCESS,

      // --- filesystem: served from the preopens, through the sync ops ---
      fd_prestat_get: (fd, prestatPtr) => {
        const entry = this.#fds.get(fd);
        // A guest walks fds upward until this reports EBADF, which is how it
        // discovers where its preopens stop.
        if (!entry || entry.kind !== "dir" || entry.preopen === undefined) {
          return ERRNO.BADF;
        }
        const view = this.#view();
        view.setUint8(prestatPtr, 0); // preopentype: dir
        view.setUint32(
          prestatPtr + 4,
          encoder.encode(entry.preopen).length,
          true,
        );
        return ERRNO.SUCCESS;
      },
      fd_prestat_dir_name: (fd, pathPtr, pathLen) => {
        const entry = this.#fds.get(fd);
        if (!entry || entry.preopen === undefined) return ERRNO.BADF;
        const name = encoder.encode(entry.preopen);
        if (name.length > pathLen) return ERRNO.INVAL;
        this.#bytes().set(name, pathPtr);
        return ERRNO.SUCCESS;
      },

      path_open: (
        dirFd,
        _dirFlags,
        pathPtr,
        pathLen,
        oflags,
        rightsBase,
        _rightsInheriting,
        fdflags,
        openedFdPtr,
      ) => {
        const host = this.#resolve(dirFd, this.#string(pathPtr, pathLen));
        if (host === null) return ERRNO.NOTCAPABLE;

        const wantsWrite =
          (BigInt(rightsBase) & (RIGHT_FD_WRITE | RIGHT_FD_ALLOCATE)) !== 0n ||
          (oflags & (OFLAGS_CREAT | OFLAGS_TRUNC)) !== 0;
        const directory = (oflags & OFLAGS_DIRECTORY) !== 0;
        const options = {
          read: true,
          write: wantsWrite,
          create: (oflags & OFLAGS_CREAT) !== 0,
          createNew: (oflags & OFLAGS_EXCL) !== 0,
          truncate: (oflags & OFLAGS_TRUNC) !== 0,
          append: (fdflags & FDFLAGS_APPEND) !== 0,
          directory,
        };

        try {
          if (directory) {
            // A directory fd is an anchor, not a stream: record the path and
            // check it really is a directory, without holding a host handle.
            const stat = __ops.sync_fs_stat(host);
            if (!stat.isDir) return ERRNO.NOTDIR;
            const fd = this.#nextFd++;
            this.#fds.set(fd, { kind: "dir", host, handle: null });
            this.#view().setUint32(openedFdPtr, fd, true);
            return ERRNO.SUCCESS;
          }
          // A read-only open must not demand FileWrite, so the two are separate
          // ops carrying separate capabilities.
          const handle = wantsWrite
            ? __ops.sync_fs_open_write(host, options)
            : __ops.sync_fs_open(host, options);
          const fd = this.#nextFd++;
          this.#fds.set(fd, { kind: "file", host, handle });
          this.#view().setUint32(openedFdPtr, fd, true);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },

      fd_filestat_get: (fd, bufPtr) => {
        const entry = this.#fds.get(fd);
        if (!entry) return ERRNO.BADF;
        try {
          const stat =
            entry.handle === null
              ? __ops.sync_fs_stat(entry.host)
              : __ops.sync_fs_fstat(entry.handle);
          this.#writeFilestat(bufPtr, stat);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },
      path_filestat_get: (dirFd, _flags, pathPtr, pathLen, bufPtr) => {
        const host = this.#resolve(dirFd, this.#string(pathPtr, pathLen));
        if (host === null) return ERRNO.NOTCAPABLE;
        try {
          this.#writeFilestat(bufPtr, __ops.sync_fs_stat(host));
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },

      fd_readdir: (fd, bufPtr, bufLen, cookie, bufusedPtr) => {
        const entry = this.#fds.get(fd);
        if (!entry || entry.kind !== "dir") return ERRNO.BADF;
        try {
          return this.#writeDirents(
            entry.host,
            bufPtr,
            bufLen,
            BigInt(cookie),
            bufusedPtr,
          );
        } catch (e) {
          return errnoFor(e);
        }
      },

      path_create_directory: (dirFd, pathPtr, pathLen) =>
        this.#pathOp(dirFd, pathPtr, pathLen, (host) => __ops.sync_fs_mkdir(host)),
      path_unlink_file: (dirFd, pathPtr, pathLen) =>
        this.#pathOp(dirFd, pathPtr, pathLen, (host) =>
          __ops.sync_fs_remove_file(host),
        ),
      path_remove_directory: (dirFd, pathPtr, pathLen) =>
        this.#pathOp(dirFd, pathPtr, pathLen, (host) =>
          __ops.sync_fs_remove_dir(host),
        ),
      path_rename: (fromFd, fromPtr, fromLen, toFd, toPtr, toLen) => {
        const from = this.#resolve(fromFd, this.#string(fromPtr, fromLen));
        const to = this.#resolve(toFd, this.#string(toPtr, toLen));
        if (from === null || to === null) return ERRNO.NOTCAPABLE;
        try {
          __ops.sync_fs_rename(from, to);
          return ERRNO.SUCCESS;
        } catch (e) {
          return errnoFor(e);
        }
      },

      // Not implemented: no host primitive behind them yet.
      fd_advise: () => ERRNO.SUCCESS, // advisory only; ignoring it is conforming
      fd_allocate: notCapable,
      fd_filestat_set_size: notCapable,
      fd_filestat_set_times: notCapable,
      fd_pread: notCapable,
      fd_pwrite: notCapable,
      fd_renumber: notCapable,
      fd_fdstat_set_rights: notCapable,
      path_filestat_set_times: notCapable,
      path_link: notCapable,
      path_readlink: notCapable,
      path_symlink: notCapable,

      // --- polling and sockets ---
      poll_oneoff: noSys,
      sock_accept: notCapable,
      sock_recv: notCapable,
      sock_send: notCapable,
      sock_shutdown: notCapable,
    };
  }
}

export default { WASI };
