// `runtime:wasi` — WASI preview 1 (`wasi_snapshot_preview1`).
//
// Enough of the ABI to run what the `wasm32-wasip1` toolchains emit for
// compute-and-print workloads: arguments, environment, clocks, randomness,
// stdio, and process exit. The filesystem calls are present but report
// `ENOTCAPABLE` — see "Filesystem" below.
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
// Preopens are not wired yet. WASI's file calls are synchronous and `runtime:fs`
// is asynchronous, so serving them needs synchronous, capability-gated host ops
// that do not exist yet; every fd beyond stdio therefore reports `ENOTCAPABLE`,
// which is the errno a WASI program is required to handle. The imports are all
// *present* regardless — a missing import is a `LinkError` at instantiation,
// which would fail a program that never calls the function.

const ERRNO = {
  SUCCESS: 0,
  BADF: 8,
  FAULT: 21,
  INVAL: 28,
  NOSYS: 52,
  NOTCAPABLE: 76,
  PERM: 63,
  SPIPE: 70,
};

/// WASI clock ids.
const CLOCK_REALTIME = 0;
const CLOCK_MONOTONIC = 1;

const FILETYPE_CHARACTER_DEVICE = 2;

/// `fd_fdstat_get` writes 24 bytes; stdio is a character device.
const FDSTAT_SIZE = 24;

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
  #exited = false;

  /**
   * @param {object} [options]
   * @param {string[]} [options.args] `argv`, including `argv[0]`.
   * @param {Record<string,string>} [options.env] Environment pairs.
   * @param {string} [options.version] Must be `"preview1"` if given.
   */
  constructor(options = {}) {
    const { args = [], env = {}, version = "preview1" } = options ?? {};
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
        this.#exited = true;
        throw new ExitStatus(code);
      },
      proc_raise: () => ERRNO.NOSYS,
      sched_yield: () => ERRNO.SUCCESS,

      // --- stdio ---
      fd_write: (fd, iovsPtr, iovsLen, nwrittenPtr) => {
        const bytes = this.#gather(iovsPtr, iovsLen);
        if (fd === 1) this.#stdout.write(bytes);
        else if (fd === 2) this.#stderr.write(bytes);
        else return ERRNO.NOTCAPABLE;
        this.#view().setUint32(nwrittenPtr, bytes.length, true);
        return ERRNO.SUCCESS;
      },
      fd_read: (fd, _iovsPtr, _iovsLen, nreadPtr) => {
        // No stdin source is wired: report a clean end-of-file rather than an
        // error, which is what a reader expects from an empty stream.
        if (fd !== 0) return ERRNO.NOTCAPABLE;
        this.#view().setUint32(nreadPtr, 0, true);
        return ERRNO.SUCCESS;
      },
      fd_fdstat_get: (fd, statPtr) => {
        if (fd > 2) return ERRNO.BADF;
        const view = this.#view();
        // Zero the struct, then set the filetype; stdio has no flags or rights
        // worth advertising beyond "it is a character device".
        for (let i = 0; i < FDSTAT_SIZE; i++) view.setUint8(statPtr + i, 0);
        view.setUint8(statPtr, FILETYPE_CHARACTER_DEVICE);
        return ERRNO.SUCCESS;
      },
      fd_close: (fd) => (fd <= 2 ? ERRNO.SUCCESS : ERRNO.BADF),
      // stdio is a pipe: seeking it is invalid, which is what ESPIPE says.
      fd_seek: (fd) => (fd <= 2 ? ERRNO.SPIPE : ERRNO.BADF),
      fd_tell: (fd) => (fd <= 2 ? ERRNO.SPIPE : ERRNO.BADF),
      fd_fdstat_set_flags: () => ERRNO.SUCCESS,
      fd_sync: () => ERRNO.SUCCESS,
      fd_datasync: () => ERRNO.SUCCESS,

      // --- filesystem: present, but no fd beyond stdio is available ---
      fd_prestat_get: notCapable,
      fd_prestat_dir_name: notCapable,
      fd_advise: notCapable,
      fd_allocate: notCapable,
      fd_filestat_get: notCapable,
      fd_filestat_set_size: notCapable,
      fd_filestat_set_times: notCapable,
      fd_pread: notCapable,
      fd_pwrite: notCapable,
      fd_readdir: notCapable,
      fd_renumber: notCapable,
      fd_fdstat_set_rights: notCapable,
      path_create_directory: notCapable,
      path_filestat_get: notCapable,
      path_filestat_set_times: notCapable,
      path_link: notCapable,
      path_open: notCapable,
      path_readlink: notCapable,
      path_remove_directory: notCapable,
      path_rename: notCapable,
      path_symlink: notCapable,
      path_unlink_file: notCapable,

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
