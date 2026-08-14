declare module "runtime:watch" {
  /**
   * What happened to a path.
   *
   * Three names, where the filesystem backends have dozens. A consumer's
   * question is "does what I cached still stand?", which has three answers; the
   * backends disagree about the rest, and a name that means one thing on Linux
   * and another on macOS is worse than no name at all.
   */
  export type ChangeKind = "created" | "modified" | "removed";

  /** One change, after the host has coalesced the burst an editor save makes. */
  export interface Change {
    kind: ChangeKind;
    /** The resolved absolute path — the same form `runtime:fs` reports. */
    path: string;
  }

  export interface WatchOptions {
    /**
     * Watch directories below the given ones too. Off by default, matching what
     * the OS watchers do natively: a recursive watch of a large tree costs a
     * descriptor per directory on Linux, and a caller watching one file should
     * not pay for the tree it sits in.
     */
    recursive?: boolean;
  }

  /**
   * A live watch: an async iterable of {@link Change}s whose set of watched
   * paths can be changed while it is being iterated.
   */
  export interface Watcher extends AsyncIterable<Change> {
    /** The next change, or `null` once the watcher is closed. */
    next(): Promise<Change | null>;
    /** Starts watching another path. `false` if it was already watched. */
    add(path: string): Promise<boolean>;
    /** Stops watching one path. `false` if it was not being watched. */
    remove(path: string): Promise<boolean>;
    /** Ends the watch and releases its descriptors. Idempotent. */
    close(): Promise<void>;
  }

  /**
   * Watches one or more paths for changes.
   *
   * **`esdev` only.** A watcher is development machinery — what it watches is
   * source — so `esrun` does not serve this module at all, and importing it
   * there fails at load rather than yielding a watcher that never fires.
   *
   * Needs the `FileRead` capability, and is bounded by the same
   * `--allow-read` list as reading: watching a directory tells you which files
   * exist and when they are touched, which is a read in every way that matters.
   *
   * Events are debounced per path, because one editor save is several
   * filesystem events and a consumer that rebuilds on each of them rebuilds
   * three times — the last two against a file that was already finished.
   *
   * ```ts
   * const changes = watch(["app"], { recursive: true });
   * for await (const { kind, path } of changes) {
   *   invalidate(path);
   *   for (const dep of rebuild()) changes.add(dep);
   * }
   * ```
   */
  export function watch(
    paths: string | readonly string[],
    options?: WatchOptions,
  ): Watcher;

  const _default: { watch: typeof watch };
  export default _default;
}
