// `import.meta.hot` — hot module replacement, as `esdev start` provides it.
//
// Not a `runtime:` module, and here for the same reason `runtime-build.d.ts` is:
// the surface exists only under `esdev`, and a project that writes against it
// still has to typecheck. `esrun` injects nothing, so in a deployed build the
// property is simply absent — which is why it is optional rather than assumed,
// and why `if (import.meta.hot)` is the shape that compiles in both.

/** What a module may do about being replaced. */
interface EsdevHot {
  /**
   * Re-run this module in place when it, or anything it imports, changes.
   *
   * With a callback, it is called after the re-run with the module's new
   * exports.
   */
  accept(callback?: (module: Record<string, unknown>) => void): void;
  /**
   * Re-run **that dependency** and tell this module, with the dependency's new
   * exports. The accepting module does not re-run — the update is delivered to
   * it, not applied to it.
   */
  accept(dependency: string, callback?: (module: Record<string, unknown>) => void): void;
  /** The same, for several dependencies. */
  accept(dependencies: string[], callback?: (module: Record<string, unknown>) => void): void;

  /**
   * Aborted immediately before this module is replaced.
   *
   * The tidiest teardown available, because the platform already takes one
   * everywhere:
   *
   * ```ts
   * addEventListener("resize", onResize, { signal: import.meta.hot.signal });
   * ```
   *
   * That listener is correct under replacement with no cleanup code, and the
   * same line is correct in a production build, where nothing aborts it.
   */
  readonly signal: AbortSignal;

  /**
   * A value made once and returned on every replacement after.
   *
   * ```ts
   * const cache = import.meta.hot.keep("cache", () => new Map());
   * ```
   *
   * One call site, where writing into `data` from `dispose` and reading it back
   * at the top of the module is two that have to agree.
   */
  keep<T>(key: string, make: () => T): T;

  /** Runs before this module is replaced, with `data`. */
  dispose(callback: (data: Record<string, unknown>) => void): void;
  /** An object that survives replacement. `keep` is usually what you want. */
  readonly data: Record<string, unknown>;

  /** Refuse replacement: any change reaching this module reloads the page. */
  decline(): void;
  /** Hand the update up to this module's importers instead. */
  invalidate(): void;

  /**
   * Called after a server restart, once the new server is listening, with
   * the servers that restarted and the changed sources, root-relative with a
   * leading `/`. The page is left alone when nothing it loads changed; this
   * is how it learns the server did.
   */
  on(event: "esdev:server-update", callback: (data: EsdevServerUpdate) => void): void;
  /** Calls `callback(data)` each time esdev sends `event`. Removed when this module is replaced. */
  on(event: string, callback: (data: unknown) => void): void;
  /** Removes a listener `on` added from this module. */
  off(event: string, callback: (data: never) => void): void;
}

/** What `esdev:server-update` carries. */
interface EsdevServerUpdate {
  /** The servers that restarted: their `dev.servers` names, or the target `dev.run` names. */
  servers: string[];
  /** The changed sources, root-relative with a leading `/`: `"/app/todos/loader.js"`. */
  files: string[];
}

interface ImportMeta {
  /**
   * Present when `esdev start` built this bundle, and absent otherwise — so
   * `if (import.meta.hot)` is how a module says "development only" and still
   * compiles for production.
   */
  readonly hot?: EsdevHot;
}
