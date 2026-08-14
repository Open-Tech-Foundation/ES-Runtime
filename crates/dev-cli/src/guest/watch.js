// runtime:watch — file-change events, for a program that has to stay up
// through them (esdev only).
//
// `esdev --watch` restarts the program on a change. This is the other half of
// that idea, for the case where restarting is the wrong answer: a dev server
// holding compiled chunks, an open websocket to a browser, and a compile
// server it took a second to warm up cannot throw all of it away because one
// file of forty changed. What it needs is to be told *which* file, so it can
// drop what depended on it and keep the rest.
//
// The set of watched paths is not fixed at construction, because it cannot be:
// which files a bundle depends on is known only once it has been built, so a
// shared `lib/` outside the app directory starts being watched the moment a
// chunk proves it needs it.
//
//   const changes = watch(["app"], { recursive: true });
//   for await (const { kind, path } of changes) {
//     invalidate(path);
//     for (const dep of newDeps) changes.add(dep);
//   }
//
// Events are debounced per path by the host: one editor save is several
// filesystem events, and rebuilding on each of them rebuilds three times.

const ops = globalThis.__ops;

// A live watcher: an async iterable of changes, whose watch set can be changed
// while it is being iterated.
class Watcher {
  constructor(ready) {
    this._ready = ready; // Promise<handle>
    this._closed = false;
    // A watch that could not be opened — a path outside the sandbox root, or
    // one --allow-read does not cover — must not also end the process as an
    // unhandled rejection. `next()` and the iterator still reject.
    this._ready.catch(() => {});
  }

  // The next change, or null once the watcher is closed.
  async next() {
    if (this._closed) return null;
    const handle = await this._ready;
    return await ops.watch_next(handle);
  }

  // Starts watching another path. Resolves to false if it was already watched.
  async add(path) {
    const handle = await this._ready;
    return await ops.watch_add(handle, String(path));
  }

  // Stops watching one path, leaving the rest of the set alone. Resolves to
  // false if it was not being watched.
  async remove(path) {
    const handle = await this._ready;
    return await ops.watch_remove(handle, String(path));
  }

  // Ends the watch and releases its descriptors. Idempotent, and safe to call
  // while something is awaiting `next()` — that call resolves to null.
  async close() {
    if (this._closed) return;
    this._closed = true;
    const handle = await this._ready;
    ops.watch_close(handle);
  }

  async *[Symbol.asyncIterator]() {
    try {
      for (;;) {
        const change = await this.next();
        if (change === null) return;
        yield change;
      }
    } finally {
      // `break` out of a for-await, or a throw inside it, must not leave the
      // OS-level watch running with nobody reading it.
      await this.close();
    }
  }
}

// Watches one or more paths. Returns immediately; the watch itself is opened
// on the first await, so a bad path surfaces where it is used.
//
//   watch("app")                        one path, non-recursive
//   watch(["app", "lib"], { recursive: true })
function watch(paths, options = {}) {
  const list = Array.isArray(paths) ? paths.map(String) : [String(paths)];
  if (list.length === 0) {
    throw new TypeError("watch: at least one path is required");
  }
  if (options === null || typeof options !== "object") {
    throw new TypeError(`watch: options must be an object, got ${typeof options}`);
  }
  // Non-recursive by default, matching what the OS watchers do natively: a
  // recursive watch of a large tree costs a descriptor per directory on Linux,
  // and a caller watching one file should not pay for the tree it sits in.
  const recursive = options.recursive === true;
  return new Watcher(ops.watch_open(list, recursive));
}

export { watch, Watcher };
export default { watch };
