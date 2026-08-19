// runtime:system — child processes (DECISIONS D37). An ES module backed by ops
// gated on Capability::Run, the grant that starts a program *outside* every
// confinement this runtime applies: no capability check, no filesystem jail, no
// execution deadline reaches a child.
//
// The shape is `Deno.Command` (a command you describe, then either `output()`
// or `spawn()`), carried on web streams like the rest of this runtime — so a
// child's stdout is a ReadableStream you can hand straight to `new Response()`,
// and a request body is something you can pipe straight into its stdin.
//
// Three deliberate differences from Node/Deno/Bun:
//
//   * No shell. There is no `exec`, no `shell: true`, no template form. A
//     command is a program plus an argv, so a guest-supplied argument is data —
//     never a second command.
//   * No inherited environment. A child gets exactly the `env` you pass. To
//     inherit, ask for it (`inheritEnv: true`), which additionally needs the Env
//     capability — so Run alone cannot hand the host's secrets to a child.
//   * `output()` is bounded. A child that writes without end hits `maxBuffer`
//     and is killed, rather than growing the server's heap until it dies.

const ops = globalThis.__ops;
const encoder = new TextEncoder();

// A Secret from runtime:process (a masked env value). It stringifies to
// "[redacted]", so passing one to a child unnoticed would send the child the
// literal mask; the symbol-keyed getter is how a deliberate reader gets the
// real value (see the note in process.js).
const SECRET_VALUE = Symbol.for("runtime.secret.value");

const STDIO_MODES = ["piped", "inherit", "null"];
// 16 MiB. Node caps `exec` at 1 MiB, Deno does not cap at all; a server runtime
// wants a limit that is generous for real output and still a limit.
const DEFAULT_MAX_BUFFER = 16 * 1024 * 1024;

function fail(message, code) {
  const err = new Error(message);
  if (code !== undefined) err.code = code;
  return err;
}

// ---- option normalization --------------------------------------------------

function checkProgram(program) {
  if (typeof program !== "string" || program.length === 0) {
    throw new TypeError("a command needs a program name");
  }
  return program;
}

// Arguments reach the child verbatim. Primitives are coerced (a port number is
// an argument like any other); anything else is a mistake worth naming, since
// "[object Object]" as an argument is never what was meant.
function checkArgs(args) {
  if (args === undefined) return [];
  if (!Array.isArray(args)) throw new TypeError("args must be an array");
  return args.map((arg, i) => {
    const type = typeof arg;
    if (type === "string") return arg;
    if (type === "number" || type === "bigint" || type === "boolean") return String(arg);
    throw new TypeError(`args[${i}] must be a string, number, or boolean`);
  });
}

function checkCwd(cwd) {
  if (cwd === undefined || cwd === null) return "";
  if (cwd instanceof URL || (typeof cwd === "string" && cwd.startsWith("file://"))) {
    const url = cwd instanceof URL ? cwd : new URL(cwd);
    if (url.protocol !== "file:") throw new TypeError(`cwd expects a file: URL, got ${url.protocol}`);
    let path = decodeURIComponent(url.pathname);
    if (/^\/[A-Za-z]:/.test(path)) path = path.slice(1); // Windows drive: /C:/x -> C:/x
    return path;
  }
  if (typeof cwd !== "string") throw new TypeError("cwd must be a string or a file: URL");
  return cwd;
}

// The child's complete environment, as the [name, value] pairs the op takes.
// `inheritEnv` reads the host environment through the Env-gated op, so a runtime
// granted Run but not Env cannot inherit — it throws where it would have leaked.
function checkEnv(env, inheritEnv) {
  const pairs = inheritEnv ? ops.process_env() : [];
  if (env === undefined || env === null) return pairs;
  if (typeof env !== "object") throw new TypeError("env must be an object");
  const merged = new Map(pairs);
  for (const [key, value] of Object.entries(env)) {
    if (value === undefined) {
      merged.delete(key); // an explicit undefined removes an inherited key
      continue;
    }
    merged.set(key, unwrapSecret(key, value));
  }
  return [...merged];
}

function unwrapSecret(key, value) {
  if (value !== null && typeof value === "object" && SECRET_VALUE in value) {
    return value[SECRET_VALUE];
  }
  if (typeof value !== "string") {
    throw new TypeError(`env.${key} must be a string`);
  }
  return value;
}

// A stdio channel is a mode name; `stdin` also accepts a body to feed the child
// (a string, bytes, Blob, Response, or ReadableStream), which is the same
// "any web body" input `runtime:fs` write() takes.
function checkStdio(name, value, fallback) {
  if (value === undefined) return fallback;
  if (typeof value === "string") {
    if (!STDIO_MODES.includes(value)) {
      throw new TypeError(`${name} must be one of ${STDIO_MODES.join(", ")}`);
    }
    return value;
  }
  throw new TypeError(`${name} must be one of ${STDIO_MODES.join(", ")}`);
}

function isMode(value) {
  return typeof value === "string" && STDIO_MODES.includes(value);
}

function isInput(value) {
  return (
    typeof value === "string" ||
    value instanceof Uint8Array ||
    ArrayBuffer.isView(value) ||
    value instanceof ArrayBuffer ||
    value instanceof Blob ||
    value instanceof Response ||
    value instanceof ReadableStream
  );
}

// Normalizes any accepted stdin input to a ReadableStream of bytes.
function inputStream(input) {
  if (input instanceof ReadableStream) return input;
  if (input instanceof Blob) return input.stream();
  if (input instanceof Response) return input.body ?? emptyStream();
  const bytes =
    typeof input === "string"
      ? encoder.encode(input)
      : input instanceof Uint8Array
        ? input
        : ArrayBuffer.isView(input)
          ? new Uint8Array(input.buffer, input.byteOffset, input.byteLength)
          : new Uint8Array(input);
  return new ReadableStream({
    start(controller) {
      if (bytes.byteLength > 0) controller.enqueue(bytes);
      controller.close();
    },
  });
}

function emptyStream() {
  return new ReadableStream({
    start(controller) {
      controller.close();
    },
  });
}

function toBytes(chunk) {
  if (chunk instanceof Uint8Array) return chunk;
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  throw new TypeError("a stdin write expects a string, ArrayBuffer, or ArrayBufferView");
}

function concat(chunks, total) {
  const out = new Uint8Array(total);
  let at = 0;
  for (const chunk of chunks) {
    out.set(chunk, at);
    at += chunk.byteLength;
  }
  return out;
}

// ---- the running child -----------------------------------------------------

// `Command.spawn` claims the child's stdin when it is feeding a body in. A
// module-scoped symbol rather than a public name: the two classes need one
// word between them, and everything else about a ChildProcess is private.
const TAKE_STDIN = Symbol("takeStdin");

// A spawned process. Its streams are the web ones: `stdout`/`stderr` are pulled
// from the host chunk by chunk (real backpressure — a child that outruns its
// reader is stopped by a full pipe, not by buffering), `stdin` is a
// WritableStream whose close() is the child's EOF.
class ChildProcess {
  #id;
  #killSignal;
  #exited;
  #released;
  #status;
  #modes;
  #openStreams;
  #streams;
  #stdinTaken;

  constructor(id, pid, options) {
    this.#id = id;
    this.pid = pid;
    this.#killSignal = options.killSignal;
    // Released to the host once the child has exited *and* nothing can still be
    // read from it. Releasing earlier would kill a live child; never releasing
    // would keep its pipes for the lifetime of the process.
    this.#exited = false;
    this.#released = false;
    this.#status = null;
    this.#modes = { stdin: options.stdin, stdout: options.stdout, stderr: options.stderr };
    // Every piped output channel must finish before the child is released, even
    // one nobody ever reads — releasing it would throw away output the guest
    // can still ask for. (Node's `exit`-before-`close` is exactly this bug.)
    this.#openStreams = [options.stdout, options.stderr].filter((m) => m === "piped").length;
    this.#streams = { stdin: null, stdout: null, stderr: null };
    // Set when a body given as `stdin` has taken the writable over.
    this.#stdinTaken = false;
  }

  // The streams are built on first use, not at spawn. A ReadableStream starts
  // pulling as soon as it exists, and an eager pull is a pending host op: it
  // would keep the whole program alive waiting for output from a child the
  // caller never intended to read.
  get stdin() {
    if (this.#modes.stdin !== "piped" || this.#stdinTaken) return null;
    this.#streams.stdin ??= this.#writable();
    return this.#streams.stdin;
  }

  get stdout() {
    return this.#output("stdout");
  }

  get stderr() {
    return this.#output("stderr");
  }

  #output(which) {
    if (this.#modes[which] !== "piped") return null;
    this.#streams[which] ??= this.#readable(which);
    return this.#streams[which];
  }

  // Resolves when the child exits. Reading it is what keeps the program alive
  // until then: a pending host op is pending work, so the runtime keeps
  // ticking. A child nobody waits on holds nothing open — spawn it, ignore it,
  // and the program can still exit (it is killed on the way out).
  get status() {
    if (this.#status === null) {
      this.#status = ops.system_wait(this.#id).then((status) => {
        this.#exited = true;
        this.#maybeRelease();
        return status;
      });
    }
    return this.#status;
  }

  // Sends `signal` (default SIGTERM). Signalling a child that has already
  // exited is a no-op, not an error — the race is unavoidable for the caller.
  //
  // Only this child is signalled. A child that spawned its own children does
  // not pass it on, so grandchildren can outlive a kill.
  kill(signal = this.#killSignal) {
    // Already exited *and* reaped: the handle has been given back, so there is
    // nothing left to signal. This is the no-op the contract above promises —
    // without the guard it became `ERR_FOREIGN_HANDLE`, which is the right
    // answer to naming someone else's child and the wrong one to naming your
    // own after it finished.
    if (this.#released) return Promise.resolve();
    return ops.system_kill(this.#id, signal);
  }

  // Strictly on demand: `highWaterMark: 0` means a read is only ever in flight
  // because somebody is waiting for it.
  //
  // The default of 1 makes a stream read one chunk *ahead* — so after a caller
  // takes the chunk it wanted, a `system_read` is left outstanding on a child
  // that may have nothing more to say. That read is pending host work, and
  // pending host work is what keeps the runtime ticking: a program that read
  // one line of a child's output, printed its summary and returned would then
  // never exit. Zero is also the honest queue depth for a pipe, whose
  // backpressure is the pipe itself.
  #readable(which) {
    const self = this;
    return new ReadableStream(
      {
        async pull(controller) {
          const chunk = await ops.system_read(self.#id, which);
          if (chunk === null) {
            controller.close();
            self.#streamDone();
          } else {
            controller.enqueue(chunk);
          }
        },
        cancel() {
          self.#streamDone();
        },
      },
      { highWaterMark: 0 },
    );
  }

  #writable() {
    const self = this;
    return new WritableStream({
      async write(chunk) {
        await ops.system_write(self.#id, toBytes(chunk));
      },
      // Closing the stdin of a child that has already been reaped is the same
      // no-op as signalling one: its pipes went with it.
      close() {
        if (self.#released) return undefined;
        return ops.system_stdin_close(self.#id);
      },
      abort() {
        if (self.#released) return undefined;
        return ops.system_stdin_close(self.#id);
      },
    });
  }

  #streamDone() {
    this.#openStreams -= 1;
    this.#maybeRelease();
  }

  // See TAKE_STDIN: `Command.spawn` is writing the body it was given, so the
  // caller's `stdin` is spent.
  [TAKE_STDIN]() {
    this.#stdinTaken = true;
  }

  #maybeRelease() {
    if (this.#released || !this.#exited || this.#openStreams > 0) return;
    this.#released = true;
    ops.system_close(this.#id);
  }

}

// `await using child = await cmd.spawn()` — kill the child and reap it when the
// scope ends, however it ends. Defined conditionally: explicit resource
// management is recent, and a runtime whose engine predates it should still
// load this module.
if (typeof Symbol.asyncDispose === "symbol") {
  ChildProcess.prototype[Symbol.asyncDispose] = async function dispose() {
    await this.kill().catch(() => {});
    await this.status.catch(() => {});
  };
}

// ---- the command -----------------------------------------------------------

class Command {
  #program;
  #args;
  #cwd;
  #env;
  #input;
  #stdin;
  #stdout;
  #stderr;
  #killSignal;
  #maxBuffer;
  #signal;

  constructor(program, options = {}) {
    if (options === null || typeof options !== "object") {
      throw new TypeError("command options must be an object");
    }
    this.#program = checkProgram(program);
    this.#args = checkArgs(options.args);
    this.#cwd = checkCwd(options.cwd);
    this.#env = checkEnv(options.env, options.inheritEnv === true);
    // stdin also accepts a body; keep it aside and pipe it in after spawning.
    // A mode name wins over the body reading, so the three mode strings are not
    // usable as literal input — pass those as bytes if you really mean them.
    this.#input = isMode(options.stdin) || !isInput(options.stdin) ? null : options.stdin;
    this.#stdin = this.#input ? "piped" : checkStdio("stdin", options.stdin, "null");
    this.#stdout = checkStdio("stdout", options.stdout, "piped");
    this.#stderr = checkStdio("stderr", options.stderr, "piped");
    this.#killSignal = options.killSignal ?? "SIGTERM";
    this.#maxBuffer = options.maxBuffer ?? DEFAULT_MAX_BUFFER;
    this.#signal = abortSignal(options.signal, options.timeout);
  }

  // Starts the child and resolves once it is running. Async — unlike Deno's
  // sync spawn() — because the host seam is: a failure to *start* (no such
  // program, permission denied) belongs to this call, not to a stream or a
  // status settled later.
  async spawn() {
    this.#signal?.throwIfAborted();
    const { id, pid } = await ops.system_spawn({
      program: this.#program,
      args: this.#args,
      cwd: this.#cwd,
      env: this.#env,
      stdin: this.#stdin,
      stdout: this.#stdout,
      stderr: this.#stderr,
    });
    const child = new ChildProcess(id, pid, {
      killSignal: this.#killSignal,
      stdin: this.#stdin,
      stdout: this.#stdout,
      stderr: this.#stderr,
    });
    if (this.#signal) watchAbort(this.#signal, child);
    if (this.#input) {
      // Feed the given body in, then close stdin. Detached: a child that stops
      // reading (a `head`, a failed filter) breaks the pipe, and that must not
      // reject in the caller's face.
      const sink = child.stdin;
      child[TAKE_STDIN](); // ours to write; the caller's `stdin` is spent
      inputStream(this.#input)
        .pipeTo(sink)
        .catch(() => {});
    }
    return child;
  }

  // Runs to completion and collects the output: the 90% case. Piped output is
  // read while the child runs — never after it exits — so a child that fills
  // its pipe cannot deadlock against a parent that is waiting for it.
  async output() {
    const child = await this.spawn();
    const abort = this.#signal;
    const [stdout, stderr, status] = await Promise.all([
      this.#collect(child, child.stdout),
      this.#collect(child, child.stderr),
      child.status,
    ]);
    // An abort races the child's own exit; if it fired, the reason is what the
    // caller asked to hear about, not the exit status that resulted from it.
    abort?.throwIfAborted();
    return { success: status.success, code: status.code, signal: status.signal, stdout, stderr };
  }

  async #collect(child, stream) {
    if (stream === null) return new Uint8Array(0);
    const reader = stream.getReader();
    const chunks = [];
    let total = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      total += value.byteLength;
      if (total > this.#maxBuffer) {
        // Stop the source of the flood, then say so. Leaving the child running
        // would keep filling a pipe nobody drains.
        await child.kill(this.#killSignal).catch(() => {});
        await reader.cancel().catch(() => {});
        throw fail(
          `the child produced more than maxBuffer (${this.#maxBuffer} bytes) of output`,
          "ERR_MAX_BUFFER",
        );
      }
    }
    return concat(chunks, total);
  }
}

// The signal that ends the child: the caller's, a timeout, or both.
function abortSignal(signal, timeout) {
  if (signal !== undefined && !(signal instanceof AbortSignal)) {
    throw new TypeError("signal must be an AbortSignal");
  }
  if (timeout === undefined) return signal ?? null;
  if (typeof timeout !== "number" || !(timeout > 0)) {
    throw new TypeError("timeout must be a positive number of milliseconds");
  }
  const deadline = AbortSignal.timeout(timeout);
  return signal ? AbortSignal.any([signal, deadline]) : deadline;
}

function watchAbort(signal, child) {
  const kill = () => {
    child.kill().catch(() => {});
  };
  if (signal.aborted) kill();
  else signal.addEventListener("abort", kill, { once: true });
}

export { Command, ChildProcess };
export default { Command, ChildProcess };
